//! Paid sites, provisional occupancy, and refunds before the first build tick.

use crate::{Building, BuildingId, Event, Order, PlayerId, State, UnitId};

impl State {
    /// Full refunds available when these workers replace their unstarted sites.
    pub fn construction_refund(&self, player: PlayerId, units: &[UnitId]) -> u32 {
        replaced_sites(self, player, units)
            .iter()
            .filter_map(|id| {
                self.building(*id)?
                    .stats()
                    .construction
                    .map(|stats| stats.cost)
            })
            .fold(0, u32::saturating_add)
    }
}

pub(crate) fn committed(unit: &crate::Unit, site: &Building) -> bool {
    unit.hp > 0
        && unit.player == site.player
        && unit.kind.stats().harvest.is_some()
        && std::iter::once(&unit.order)
            .chain(&unit.queue)
            .any(|order| {
                matches!(order, Order::Build { site: id } if *id == site.id)
                    || matches!(order, Order::Found { kind, anchor }
                    if *kind == site.kind && *anchor == site.anchor)
            })
}

pub(crate) fn replaced_sites(state: &State, player: PlayerId, units: &[UnitId]) -> Vec<BuildingId> {
    state
        .buildings
        .iter()
        .filter(|site| {
            site.player == player
                && !site.built
                && site.tier == 0
                && site.progress == 0
                && state
                    .units
                    .iter()
                    .any(|unit| units.contains(&unit.id) && committed(unit, site))
                && !state
                    .units
                    .iter()
                    .any(|unit| !units.contains(&unit.id) && committed(unit, site))
        })
        .map(|site| site.id)
        .collect()
}

pub(super) fn refund(state: &mut State, id: BuildingId, events: &mut Vec<Event>) {
    let site = state.building(id).expect("collected site");
    let (player, cost) = (
        site.player,
        site.stats().construction.expect("constructible site").cost,
    );
    super::commands::cancel_site(state, player, id, cost, events);
}

pub(super) fn cancel_abandoned(state: &mut State, events: &mut Vec<Event>) {
    let abandoned: Vec<_> = state
        .buildings
        .iter()
        .filter(|site| {
            site.hp > 0
                && !site.built
                && site.tier == 0
                && site.progress == 0
                && !state.units.iter().any(|unit| committed(unit, site))
        })
        .map(|site| site.id)
        .collect();
    for id in abandoned {
        refund(state, id, events);
    }
}

pub(super) fn reveal(state: &mut State, events: &mut Vec<Event>) {
    let visible: Vec<_> = state
        .buildings
        .iter()
        .filter(|site| {
            site.provisional
                && site
                    .tiles()
                    .all(|tile| state.vision(site.player).visible(tile))
        })
        .map(|site| site.id)
        .collect();
    for id in visible {
        let site = state.building(id).expect("collected site");
        let (player, kind, anchor) = (site.player, site.kind, site.anchor);
        if state
            .place_refusal_except(player, kind, anchor, Some(id))
            .is_some()
        {
            refund(state, id, events);
            continue;
        }
        state.building_mut(id).expect("collected site").provisional = false;
        let index = state
            .buildings
            .iter()
            .position(|site| site.id == id)
            .expect("collected site");
        state.stamp_building_occupancy(index, true);
        if let Some(builder) = state
            .units
            .iter()
            .find(|unit| committed(unit, state.building(id).expect("site")))
            .map(|unit| unit.id)
        {
            super::commands::finish_site_claim(state, id, builder);
        }
        for unit in state.units.iter_mut().filter(|unit| unit.player == player) {
            if unit.order == (Order::Found { kind, anchor }) {
                unit.order = Order::Build { site: id };
                unit.path = None;
                unit.progress = 0;
            }
            for order in &mut unit.queue {
                if *order == (Order::Found { kind, anchor }) {
                    *order = Order::Build { site: id };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
