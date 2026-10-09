//! Production queues, recurring income, and abandoned-site decay.
//!
//! One queue per building, front item in progress. A finished ground unit
//! spawns on a passable ring tile in the footprint's map-relative outward
//! direction; if the ring is fully blocked the unit waits at 100% until a tile
//! opens up. Airworks aircraft instead spawn above the roof bay at the
//! building's center. A rally point, when set, hands the newborn its first
//! order.

use super::{rect_adjacent_tiles, spawn_doorstep_key};
use crate::event::Event;
use crate::ids::{PlayerId, UnitId};
use crate::state::{Order, State};
use crate::stats::{BuildingKind, Domain};
use chassis::grid::TilePos;

/// Arms each newly stranded seat from the state that existed at the tick
/// boundary. Commands run afterward, so spending or reshaping a queue on
/// the first recovery tick cannot enlarge the captured entitlement.
pub(super) fn capture_recovery_entitlements(state: &mut State) {
    let players: Vec<PlayerId> = state
        .players
        .iter()
        .enumerate()
        .map(|(index, _)| PlayerId::from_index(index))
        .collect();
    for player in players {
        if !super::harvester_recovery_needed(state, player) || !state.player(player).recovery_ready
        {
            continue;
        }
        let target = state.recovery_package_target(player);
        let allowance = target.saturating_sub(state.player(player).scrap);
        let seat = state.player_mut(player);
        seat.recovery_target =
            u16::try_from(target).expect("a recovery package fits the u16 ledger");
        seat.recovery_allowance =
            u16::try_from(allowance).expect("an allowance never exceeds its target");
        seat.recovery_ready = false;
    }
}

pub(super) fn run(state: &mut State, events: &mut Vec<Event>) {
    // Every built Reclaimer credits one scrap per period. Credits are
    // per-player sums, so order does not matter.
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
    // A restored Extractor pays a fixed remote yield. A completed own
    // Foundry close to its footprint raises that yield; support is binary
    // rather than one bonus per Foundry.
    if completed_ticks.is_multiple_of(crate::stats::EXTRACTOR_REMOTE_YIELD.1) {
        let credits: Vec<(PlayerId, u32)> = state
            .buildings
            .iter()
            .filter(|building| !state.player(building.player).resigned)
            .filter_map(|building| {
                let income = state.extractor_income(building.id)?;
                let (amount, period) = income.yield_cadence();
                completed_ticks
                    .is_multiple_of(period)
                    .then_some((building.player, amount))
            })
            .collect();
        for (player, amount) in credits {
            let bank = &mut state.player_mut(player).scrap;
            *bank = bank.saturating_add(amount);
        }
    }

    // The income floor: every completed Foundry credits a slow trickle, so
    // a living seat always has some income even with every node exhausted
    // or camped.
    if completed_ticks >= crate::stats::FOUNDRY_DRIP_START_TICK
        && completed_ticks.is_multiple_of(crate::stats::FOUNDRY_DRIP_PERIOD)
    {
        let credits: Vec<(PlayerId, u32)> = state
            .players
            .iter()
            .enumerate()
            .map(|(index, _)| PlayerId::from_index(index))
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
                    .count();
                (
                    player,
                    u32::try_from(foundries).expect("building counts fit in u32"),
                )
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
        .map(|(index, _)| PlayerId::from_index(index))
        .collect();
    for player in &players {
        if !super::harvester_recovery_needed(state, *player) || state.player(*player).recovery_ready
        {
            continue;
        }
        let seat = state.player_mut(*player);
        let headroom = u32::from(seat.recovery_target).saturating_sub(seat.scrap);
        seat.recovery_allowance = seat
            .recovery_allowance
            .min(u16::try_from(headroom).expect("headroom never exceeds the u16 target"));
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
        // Ready — aircraft occupy the open Airworks roof bay itself. Ground
        // production still needs a passable doorstep outside the footprint.
        let (anchor, size, player, rally, producer, center) = (
            b.anchor,
            b.stats().size,
            b.player,
            b.rally,
            b.kind,
            b.center(),
        );
        let domain = kind.stats().domain;
        let map_size = (state.map.width(), state.map.height());
        let spawn = if producer == BuildingKind::Airworks && domain == Domain::Air {
            Some(center)
        } else {
            rect_adjacent_tiles(anchor, size)
                .filter(|&tile| state.passable_for(domain, tile))
                .min_by_key(|&tile| spawn_doorstep_key(map_size, anchor, size, tile))
                .map(TilePos::center)
        };
        let Some(spawn) = spawn else {
            continue; // fully walled in; retry next tick
        };
        let unit = state.spawn_unit(player, kind, spawn);
        events.push(Event::UnitTrained {
            building: id,
            unit,
            kind,
            player,
        });
        if let Some(rally) = rally {
            let order = rally_order(state, unit, rally);
            state.unit_mut(unit).expect("just spawned").order = order;
        }
        let b = state.building_mut(id).expect("still standing");
        b.queue.pop_front();
        b.progress = 0;
    }
}

/// What a rally means to the fresh unit `newborn`: harvesters mine a
/// rallied node, fighters hunt, everyone else walks. The walk
/// resolves the rally like a move of the newborn's own, so it heads for the
/// rally tile until its owner's team has explored it, and an unreachable
/// rally ends as close as it can get.
///
/// A rally is clamped onto the map first, since an off-map tile could never
/// be explored.
///
/// "Node" is judged by the owner's remembered scrap and wreck, not the live
/// map: memory refreshes while the ground is visible and freezes when sight
/// is lost, so a rally can neither probe unexplored tiles nor know a
/// distant node ran dry. The newborn walks out and discovers the truth.
fn rally_order(state: &State, newborn: UnitId, rally: TilePos) -> Order {
    let unit = state.unit(newborn).expect("just spawned");
    let (owner, stats) = (unit.player, unit.kind.stats());
    let rally = TilePos::new(
        rally.x.clamp(0, state.map.width() - 1),
        rally.y.clamp(0, state.map.height() - 1),
    );
    if stats.harvest.is_some()
        && (state.vision(owner).remembered_scrap(rally) > 0
            || state.vision(owner).remembered_wreck(rally) > 0)
    {
        return Order::Harvest {
            node: rally,
            anchor: rally,
            retiring: false,
        };
    }
    let reverse = super::goals::spread_scan_reversed(state, rally, &[newborn]);
    let goal = super::goals::issue(state, owner, rally, stats.domain, reverse).goal(0);
    if stats.can_fight() {
        Order::Hunt { goal }
    } else {
        Order::Run { goal }
    }
}

/// Abandoned construction sites decay.
///
/// A site with no live own harvest-capable machine committed to build it or
/// standing beside its footprint loses one hp per
/// [`crate::stats::SITE_DECAY_PERIOD`] ticks. A queued Build order is a
/// commitment too: sites waiting behind earlier work are not abandoned.
/// Upgrading buildings (tier above zero) never decay. Survival counts
/// Foundry sites like standing Foundries, so an untended scaffold must
/// eventually die rather than keep a beaten seat alive; decay also burns
/// the cancel refund like enemy fire does. A site that reaches zero
/// resolves through cleanup with the ordinary destroyed-building rules the
/// same tick.
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
        .filter(|building| {
            !building.built && !building.provisional && building.hp > 0 && building.tier == 0
        })
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
