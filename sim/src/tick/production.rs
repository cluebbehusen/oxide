//! Phase 2: Foundry production queues.
//!
//! One queue per building, front item in progress. A finished unit spawns on
//! the first passable ring tile (row-major scan — deterministic); if the
//! ring is fully blocked the unit waits at 100% until a tile opens up. A
//! rally point, when set, hands the newborn its first order.

use super::rect_adjacent_tiles;
use crate::event::Event;
use crate::ids::PlayerId;
use crate::state::{Order, State};
use crate::stats::UnitKind;
use chassis::grid::TilePos;

/// Arms each newly stranded seat from the state that existed at the tick
/// boundary. Commands run afterward, so spending or reshaping a queue on
/// the first recovery tick cannot enlarge the captured entitlement.
pub(super) fn capture_recovery_entitlements(state: &mut State) {
    let players: Vec<PlayerId> = state
        .players
        .iter()
        .enumerate()
        .map(|(index, _)| PlayerId(index as u8))
        .collect();
    for player in players {
        if !super::harvester_recovery_needed(state, player) || !state.player(player).recovery_ready
        {
            continue;
        }
        let target = state.recovery_package_target(player);
        let allowance = target.saturating_sub(state.player(player).scrap);
        let seat = state.player_mut(player);
        seat.recovery_target = target as u16;
        seat.recovery_allowance = allowance as u16;
        seat.recovery_ready = false;
    }
}

pub(super) fn run(state: &mut State, events: &mut Vec<Event>) {
    // Reclaimers trickle first: every built one grinds ambient debris
    // into a scrap each period. Order is building-id order (commutative
    // anyway — the credits are per-player sums).
    for (period, tier) in [
        (crate::stats::RECLAIMER_PERIOD, 0u8),
        (crate::stats::REFINERY_PERIOD, 1u8),
    ] {
        if !state.tick.is_multiple_of(period) {
            continue;
        }
        let credits: Vec<PlayerId> = state
            .buildings
            .iter()
            .filter(|b| {
                b.built
                    && b.hp > 0
                    && b.kind == crate::stats::BuildingKind::Reclaimer
                    && b.tier == tier
            })
            .map(|b| b.player)
            .collect();
        for player in credits {
            let bank = &mut state.player_mut(player).scrap;
            *bank = bank.saturating_add(1);
        }
    }

    let completed_ticks = state.tick.saturating_add(1);
    // Restored Extractors grind the deep seams on the escalating
    // schedule — the visible late-game pressure rule. The applicable
    // row is the last whose start the clock has passed.
    let (_, amount, period) = crate::stats::EXTRACTOR_YIELD_SCHEDULE
        .iter()
        .rev()
        .find(|(from, ..)| completed_ticks >= *from)
        .expect("the schedule's first row starts at zero");
    if completed_ticks.is_multiple_of(*period) {
        let credits: Vec<(PlayerId, u32)> = state
            .players
            .iter()
            .enumerate()
            .map(|(index, _)| PlayerId(index as u8))
            .filter(|player| !state.player(*player).resigned)
            .map(|player| {
                let extractors = state
                    .buildings
                    .iter()
                    .filter(|building| {
                        building.player == player
                            && building.hp > 0
                            && building.built
                            && building.kind == crate::stats::BuildingKind::Extractor
                    })
                    .count() as u32;
                (player, extractors)
            })
            .filter(|(_, extractors)| *extractors > 0)
            .collect();
        for (player, extractors) in credits {
            let bank = &mut state.player_mut(player).scrap;
            *bank = bank.saturating_add(extractors.saturating_mul(*amount));
        }
    }

    // The transparent income floor: every standing Foundry smelts a slow
    // trickle per works rather than per player, so expansion bases earn
    // their keep — while the rate keeps income alone from ever paying
    // for one. A living seat always has a way back into the game, even
    // with every node exhausted and camped.
    if completed_ticks >= crate::stats::FOUNDRY_DRIP_START_TICK
        && completed_ticks.is_multiple_of(crate::stats::FOUNDRY_DRIP_PERIOD)
    {
        let credits: Vec<(PlayerId, u32)> = state
            .players
            .iter()
            .enumerate()
            .map(|(index, _)| PlayerId(index as u8))
            .filter(|player| !state.player(*player).resigned)
            .map(|player| {
                let foundries = state
                    .buildings
                    .iter()
                    .filter(|building| {
                        building.player == player
                            && building.hp > 0
                            && building.built
                            && building.kind == crate::stats::BuildingKind::Foundry
                    })
                    .count() as u32;
                (player, foundries)
            })
            .filter(|(_, foundries)| *foundries > 0)
            .collect();
        for (player, foundries) in credits {
            let bank = &mut state.player_mut(player).scrap;
            *bank = bank.saturating_add(foundries);
        }
    }

    // External income closes the captured deficit without creating new
    // emergency headroom. Spending, cancelling, or losing the package
    // never enlarges its allowance; only a real deposit re-arms a cycle.
    let players: Vec<PlayerId> = state
        .players
        .iter()
        .enumerate()
        .map(|(index, _)| PlayerId(index as u8))
        .collect();
    for player in &players {
        if !super::harvester_recovery_needed(state, *player) || state.player(*player).recovery_ready
        {
            continue;
        }
        let seat = state.player_mut(*player);
        let headroom = u32::from(seat.recovery_target).saturating_sub(seat.scrap);
        seat.recovery_allowance = seat.recovery_allowance.min(headroom as u16);
    }

    if state
        .tick
        .is_multiple_of(crate::stats::FOUNDRY_RECOVERY_PERIOD)
    {
        for player in players {
            if !super::harvester_recovery_needed(state, player) {
                continue;
            }
            let seat = state.player_mut(player);
            if seat.recovery_allowance == 0 || seat.scrap >= u32::from(seat.recovery_target) {
                continue;
            }
            seat.scrap = seat.scrap.saturating_add(1);
            seat.recovery_allowance -= 1;
        }
    }

    let ids: Vec<_> = state.buildings.iter().map(|b| b.id).collect();
    for id in ids {
        let Some(b) = state.building_mut(id) else {
            continue;
        };
        if !b.built {
            continue; // a site's progress belongs to its builder
        }
        let Some(&kind) = b.queue.front() else {
            b.progress = 0;
            continue;
        };
        b.progress = (b.progress + 1).min(kind.stats().train_ticks);
        if b.progress < kind.stats().train_ticks {
            continue;
        }
        // Ready — look for a doorstep tile (any in-bounds tile serves a
        // flyer; the ground ring can be walled shut).
        let (anchor, size, player, rally) = (b.anchor, b.stats().size, b.player, b.rally);
        let domain = kind.stats().domain;
        let spawn = rect_adjacent_tiles(anchor, size).find(|&t| state.passable_for(domain, t));
        let Some(tile) = spawn else {
            continue; // fully walled in; retry next tick
        };
        let unit = state.spawn_unit(player, kind, tile.center());
        events.push(Event::UnitTrained { unit, kind, player });
        if let Some(rally) = rally
            && let Some(order) = rally_order(state, player, kind, rally)
            && let Some(newborn) = state.unit_mut(unit)
        {
            newborn.order = order;
        }
        let b = state.building_mut(id).expect("still standing");
        b.queue.pop_front();
        b.progress = 0;
    }
}

/// What a rally means to a fresh unit: harvesters mine a rallied node,
/// fighters attack-move, everyone else walks. `None` (unwalkable rally
/// area) leaves the unit idle at the doorstep.
///
/// "Node" is judged by the owner's *remembered* scrap, not the live map —
/// it refreshes while the ground is visible and freezes when sight is
/// lost, so a rally can neither probe unexplored tiles nor know a distant
/// node ran dry. Stale beliefs resolve honestly: the newborn walks out
/// and discovers.
fn rally_order(state: &State, owner: PlayerId, kind: UnitKind, rally: TilePos) -> Option<Order> {
    let stats = kind.stats();
    if stats.harvest.is_some()
        && (state.vision(owner).remembered_scrap(rally) > 0
            || state.vision(owner).remembered_wreck(rally) > 0)
    {
        return Some(Order::Harvest {
            node: rally,
            anchor: Some(rally),
            retiring: false,
        });
    }
    let goal = super::domain_goal(state, rally, stats.domain)?;
    Some(if stats.can_fight() {
        Order::AttackMove { goal }
    } else {
        Order::Move { goal }
    })
}

/// Phase 3.5: abandoned construction sites rust away.
///
/// A site with no live own harvest-capable machine committed to build it or
/// standing beside its footprint loses one hp per
/// [`crate::stats::SITE_DECAY_PERIOD`] ticks. A queued Build order is a
/// commitment too: sites waiting behind earlier work are not abandoned.
/// Tiered works are committed self-upgrades rather than abandoned sites and
/// never enter this decay pass.
/// Survival counts Foundry sites exactly like standing Foundries, so an
/// untended scaffold must eventually die rather than keep a beaten seat
/// technically alive — and decay burns the cancel refund exactly like
/// enemy fire does. A site that reaches zero resolves through cleanup
/// with the ordinary destroyed-building rules the same tick.
pub(super) fn decay_abandoned_sites(state: &mut State) {
    if !state
        .tick
        .saturating_add(1)
        .is_multiple_of(crate::stats::SITE_DECAY_PERIOD)
    {
        return;
    }
    let decays: Vec<crate::ids::BuildingId> = state
        .buildings
        .iter()
        .filter(|building| !building.built && building.hp > 0 && building.tier == 0)
        .filter(|building| {
            !state.units.iter().any(|unit| {
                unit.player == building.player
                    && unit.hp > 0
                    && unit.kind.stats().harvest.is_some()
                    && (matches!(unit.order, Order::Build { site } if site == building.id)
                        || unit.queue.iter().any(
                            |order| matches!(*order, Order::Build { site } if site == building.id),
                        )
                        || super::tile_adjacent_to_rect(
                            unit.tile(),
                            building.anchor,
                            building.stats().size,
                        ))
            })
        })
        .map(|building| building.id)
        .collect();
    for id in decays {
        if let Some(building) = state.building_mut(id) {
            building.hp = building.hp.saturating_sub(1);
        }
    }
}
