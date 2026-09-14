//! Buried-charge triggers and the lifecycle of construction over hidden mines.

use crate::stats::{
    BuildingKind, CHARGE_BLAST_RADIUS, CHARGE_DAMAGE, CHARGE_TRIGGER_RADIUS, Domain,
};
use crate::{BuildingId, Event, Order, PlayerId, State};
use chassis::grid::TilePos;

fn known_charge_in(state: &State, player: PlayerId, kind: BuildingKind, anchor: TilePos) -> bool {
    let (w, h) = kind.base_stats().size;
    (0..h).any(|dy| (0..w).any(|dx| state.known_charge_at(player, anchor.offset(dx, dy))))
}

pub(super) fn cancel_discovered(state: &mut State, events: &mut Vec<Event>) -> bool {
    let sites: Vec<_> = state
        .buildings
        .iter()
        .filter(|b| {
            b.hp > 0
                && !b.built
                && b.tier == 0
                && b.progress == 0
                && known_charge_in(state, b.player, b.kind, b.anchor)
        })
        .map(|b| (b.id, b.player, b.stats().construction.expect("site").cost))
        .collect();
    let removed = !sites.is_empty();
    for (site, player, cost) in sites {
        super::commands::cancel_site(state, player, site, cost, events);
    }
    let mut claims = Vec::new();
    for u in state.units.iter().filter(|u| u.hp > 0) {
        for order in std::iter::once(&u.order).chain(&u.queue) {
            if let Order::Found { kind, anchor } = *order
                && known_charge_in(state, u.player, kind, anchor)
                && !claims.contains(&(u.player, kind, anchor))
            {
                claims.push((u.player, kind, anchor));
            }
        }
    }
    for (player, kind, anchor) in claims {
        let _ = super::commands::apply_cancel_found(state, player, kind, anchor);
    }
    removed
}

/// Called after the volley and before any hp work is applied. A mine killed
/// by that volley or an earlier blast is destroyed without firing.
pub(super) fn detonate_under_construction(
    state: &mut State,
    starts: &[BuildingId],
    events: &mut Vec<Event>,
) {
    if starts.is_empty() {
        return;
    }
    let triggers: Vec<_> = state
        .buildings
        .iter()
        .enumerate()
        .filter(|(_, mine)| mine.kind == BuildingKind::ScuttleCharge && mine.built && mine.hp > 0)
        .filter_map(|(slot, mine)| {
            let sites: Vec<_> = starts
                .iter()
                .filter_map(|&id| state.building(id))
                .filter(|b| {
                    b.hp > 0 && state.hostile(mine.player, b.player) && b.contains(mine.anchor)
                })
                .map(|b| (b.id, b.player))
                .collect();
            (!sites.is_empty()).then_some((slot, sites))
        })
        .collect();
    for (slot, sites) in triggers {
        if state.buildings[slot].hp == 0 {
            continue;
        }
        detonate(state, slot, events);
        for (id, player) in sites {
            state
                .building_mut(id)
                .expect("site survives until cleanup")
                .hp = 0;
            super::commands::clear_site_orders(state, player, id);
        }
    }
}

pub(super) fn detonate_under_units(state: &mut State, events: &mut Vec<Event>) {
    let trigger_sq = CHARGE_TRIGGER_RADIUS * CHARGE_TRIGGER_RADIUS;
    for slot in 0..state.buildings.len() {
        let b = &state.buildings[slot];
        if b.kind != BuildingKind::ScuttleCharge || !b.built || b.hp == 0 {
            continue;
        }
        let tripped = state.units.iter().any(|u| {
            u.hp > 0
                && state.hostile(b.player, u.player)
                && u.domain() == Domain::Ground
                && u.pos.dist_sq(b.center()) <= trigger_sq
        });
        if tripped {
            detonate(state, slot, events);
        }
    }
}

fn detonate(state: &mut State, slot: usize, events: &mut Vec<Event>) {
    let b = &state.buildings[slot];
    let (id, owner, center) = (b.id, b.player, b.center());
    let blast_sq = CHARGE_BLAST_RADIUS * CHARGE_BLAST_RADIUS;
    crate::vision::forget_observed_building(state, id, true);
    state.buildings[slot].hp = 0;
    events.push(Event::ChargeDetonated {
        building: id,
        player: owner,
        at: center,
    });
    for u in &mut state.units {
        if u.hp > 0
            && state.players[owner.0 as usize].team != state.players[u.player.0 as usize].team
            && u.domain() == Domain::Ground
            && u.pos.dist_sq(center) <= blast_sq
        {
            u.hp = u.hp.saturating_sub(CHARGE_DAMAGE);
        }
    }
    for other in &mut state.buildings {
        if other.hp > 0
            && other.kind.is_stealthy()
            && state.players[owner.0 as usize].team != state.players[other.player.0 as usize].team
            && other.center().dist_sq(center) <= blast_sq
        {
            other.hp = other.hp.saturating_sub(CHARGE_DAMAGE);
        }
    }
}
