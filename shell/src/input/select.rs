//! Picking and selection: click, box, double-click-kind, and the idle
//! harvester cycle. Any owner's visible units are selectable (allies for
//! reading, enemies for inspection), but a selection holds one owner:
//! picks and merges of a different owner replace it, since a mixed
//! own+ally selection would refuse every command under own-gating. Fog
//! and ownership rules for orders live in `orders`.

use super::Pointer;
use crate::game::Game;
use crate::numeric;
use macroquad::prelude::{Vec2, vec2};
use oxide_sim::UnitId;

/// Own harvesters with nothing to do, in id order — the cycle key and
/// the HUD badge both read this.
pub fn idle_harvesters(game: &crate::game::Scene<'_>) -> Vec<UnitId> {
    game.state
        .units()
        .iter()
        .filter(|u| {
            u.player == game.presentation.human
                && u.kind.stats().harvest.is_some()
                && u.order == oxide_sim::Order::Idle
        })
        .map(|u| u.id)
        .collect()
}

/// Selects the next idle harvester after the current selection (id
/// order, wrapping) and centers the camera on it. Stateless: the
/// selection itself is the cursor.
pub(super) fn cycle_idle_worker(game: &mut Game) {
    let idle = idle_harvesters(&game.view());
    let Some(&first) = idle.first() else {
        game.presentation.toast("No idle harvesters");
        return;
    };
    let next = match game.presentation.selection.units.as_slice() {
        [current] => idle
            .iter()
            .copied()
            .find(|id| id > current)
            .unwrap_or(first),
        _ => first,
    };
    game.presentation.selection.units = vec![next];
    game.presentation.selection.buildings.clear();
    let unit = game.state.unit(next).expect("listed above");
    game.presentation.camera.center = vec2(unit.pos.x.to_num::<f32>(), unit.pos.y.to_num::<f32>());
    game.presentation.camera.pan(Vec2::ZERO); // re-clamp
}

/// World-space pick radius around a unit: never below the unit's own
/// radius, and wider when zoomed out so it keeps a screen-space floor that
/// is larger for a fingertip.
fn pick_radius(game: &Game, ui: f32, kind: oxide_sim::UnitKind, pointer: Pointer) -> f32 {
    let reach = match pointer {
        Pointer::Mouse => 10.0,
        Pointer::Touch => 22.0,
    };
    (reach * ui / game.presentation.camera.zoom).max(super::unit_pick_radius(kind))
}

/// What a press would select.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Picked {
    Unit(UnitId, oxide_sim::PlayerId),
    Building(oxide_sim::BuildingId, oxide_sim::PlayerId),
}

/// The entity a press at `screen` picks: the nearest selectable unit in
/// reach (own units outrank foreign ones, so a scrum never steals the
/// press from the machine you command), else a selectable building
/// under it.
pub(super) fn pick(game: &Game, screen: Vec2, ui: f32, pointer: Pointer) -> Option<Picked> {
    let world = game.presentation.camera.to_world(screen);
    let tile = numeric::tile_at(world);
    // Over known salvage only a unit's body counts: the slop that widens
    // a fingertip or a zoomed-out cursor must not hand the harvester
    // working a pile the tap aimed at the pile itself.
    let salvage_here = game.view().known_salvage(tile).is_some();
    let unit = game
        .state
        .units()
        .iter()
        .filter(|u| selectable(game, u))
        .filter_map(|u| {
            let p = vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
            let distance = p.distance(world);
            let reach = if salvage_here {
                super::unit_pick_radius(u.kind)
            } else {
                pick_radius(game, ui, u.kind, pointer)
            };
            (distance <= reach).then_some((
                u.player != game.presentation.human,
                distance,
                u.id,
                u.player,
            ))
        })
        .min_by(|a, b| (a.0, a.1).partial_cmp(&(b.0, b.1)).expect("finite"));
    if let Some((_, _, id, owner)) = unit {
        return Some(Picked::Unit(id, owner));
    }
    // Only the human's own buildings skip the sight check: built ally
    // buildings are always inside shared team sight anyway, but ally
    // sites are blind until built, and selecting one through fog would
    // leak its live kind and hp through the panel.
    game.state
        .buildings_at(tile)
        .find(|b| selectable_building(game, b))
        .map(|b| Picked::Building(b.id, b.player))
}

/// Whether HUD chrome swallows a click at `screen`, per the published
/// HUD layout.
pub(super) fn click_on_hud(game: &Game, screen: Vec2) -> bool {
    game.presentation.layout.get().chrome_owns(screen)
}

/// Whether the human may see this unit at all: own and allies always
/// (team sight), enemies only on currently visible ground. Selection
/// must never reach through fog.
fn selectable(game: &Game, unit: &oxide_sim::Unit) -> bool {
    !game.state.hostile(game.presentation.human, unit.player)
        || game.presentation.all_seeing()
        || game.my_vision().visible(unit.tile())
}

/// Whether the human may select this building without learning through
/// fog — or through stealth: an undetected buried charge must not be
/// clickable on ground the player merely sees.
fn selectable_building(game: &Game, building: &oxide_sim::Building) -> bool {
    building.player == game.presentation.human
        || game.presentation.all_seeing()
        || (building.tiles().any(|tile| game.my_vision().visible(tile))
            && game
                .state
                .building_apparent(game.presentation.human, building))
}

pub(super) fn click_select(
    game: &mut Game,
    screen: Vec2,
    additive: bool,
    ui: f32,
    pointer: Pointer,
) {
    let picked = pick(game, screen, ui, pointer);
    if !additive {
        game.presentation.selection.buildings.clear();
    }
    if picked.is_some() {
        game.presentation.selection.pile = None;
    }
    if let Some(Picked::Unit(id, owner)) = picked {
        game.presentation.selection.buildings.clear();
        let current_owner = game
            .presentation
            .selection
            .units
            .first()
            .and_then(|u| game.state.unit(*u))
            .map(|u| u.player);
        if additive && current_owner == Some(owner) {
            // Shift-click toggles membership within one allegiance.
            if let Some(index) = game
                .presentation
                .selection
                .units
                .iter()
                .position(|u| *u == id)
            {
                game.presentation.selection.units.remove(index);
            } else {
                game.presentation.selection.units.push(id);
            }
        } else {
            // A different owner replaces the selection, keeping it to one
            // allegiance.
            game.presentation.selection.units = vec![id];
        }
        return;
    }
    // …then a building under the cursor (any owner whose ground shows).
    if let Some(Picked::Building(id, owner)) = picked {
        game.presentation.selection.units.clear();
        let current_owner = game
            .presentation
            .selection
            .buildings
            .first()
            .and_then(|id| game.state.building(*id))
            .map(|selected| selected.player);
        if additive && current_owner == Some(owner) {
            if let Some(index) = game
                .presentation
                .selection
                .buildings
                .iter()
                .position(|selected| *selected == id)
            {
                game.presentation.selection.buildings.remove(index);
            } else {
                game.presentation.selection.buildings.push(id);
                game.presentation.selection.buildings.sort_unstable();
            }
        } else {
            game.presentation.selection.buildings = vec![id];
        }
        return;
    }
    if additive {
        return; // shift-miss leaves the selection alone
    }
    // …otherwise inspect the salvage under it, or clear.
    game.presentation.selection.units.clear();
    game.presentation.selection.buildings.clear();
    let world = game.presentation.camera.to_world(screen);
    let tile = numeric::tile_at(world);
    game.presentation.selection.pile = game.view().known_salvage(tile).is_some().then_some(tile);
}

pub(super) fn box_select(game: &mut Game, a_screen: Vec2, b_screen: Vec2, additive: bool) {
    let a = game.presentation.camera.to_world(a_screen);
    let b = game.presentation.camera.to_world(b_screen);
    let (lo, hi) = (a.min(b), a.max(b));
    let unit_inside = |u: &&oxide_sim::Unit| {
        let p = vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
        p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y
    };
    // Commandable subjects win before foreign inspection: own units,
    // then own buildings. Within one allegiance, units retain marquee
    // priority so the selection never mixes mobile and static subjects.
    let mut boxed: Vec<UnitId> = game
        .state
        .units()
        .iter()
        .filter(|u| u.player == game.presentation.human)
        .filter(unit_inside)
        .map(|u| u.id)
        .collect();
    if !boxed.is_empty() {
        if additive {
            boxed.extend(
                game.presentation
                    .selection
                    .units
                    .iter()
                    .copied()
                    .filter(|id| {
                        game.state
                            .unit(*id)
                            .is_some_and(|u| u.player == game.presentation.human)
                    }),
            );
            boxed.sort_unstable();
            boxed.dedup();
        }
        game.presentation.selection.units = boxed;
        game.presentation.selection.buildings.clear();
        return;
    }

    // Building centers are the pick points, matching units and avoiding
    // edge-only grabs of a large footprint.
    let building_inside = |building: &&oxide_sim::Building| {
        let center = building.center();
        let p = vec2(center.x.to_num::<f32>(), center.y.to_num::<f32>());
        p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y
    };
    let mut buildings: Vec<_> = game
        .state
        .buildings()
        .iter()
        .filter(|building| building.player == game.presentation.human)
        .filter(building_inside)
        .map(|building| building.id)
        .collect();
    if !buildings.is_empty() {
        if additive {
            buildings.extend(
                game.presentation
                    .selection
                    .buildings
                    .iter()
                    .copied()
                    .filter(|id| {
                        game.state
                            .building(*id)
                            .is_some_and(|building| building.player == game.presentation.human)
                    }),
            );
            buildings.sort_unstable();
            buildings.dedup();
        }
        game.presentation.selection.units.clear();
        game.presentation.selection.buildings = buildings;
        return;
    }

    // With nothing commandable inside, inspect one visible foreign owner
    // at a time. Units still outrank buildings within that owner-neutral
    // inspection fallback.
    let foreign_unit_owner = game
        .state
        .units()
        .iter()
        .filter(|unit| unit.player != game.presentation.human && selectable(game, unit))
        .filter(unit_inside)
        .map(|unit| unit.player)
        .min();
    if let Some(owner) = foreign_unit_owner {
        let mut units: Vec<_> = game
            .state
            .units()
            .iter()
            .filter(|unit| unit.player == owner && selectable(game, unit))
            .filter(unit_inside)
            .map(|unit| unit.id)
            .collect();
        if additive {
            let current_owner = game
                .presentation
                .selection
                .units
                .first()
                .and_then(|id| game.state.unit(*id))
                .map(|unit| unit.player);
            if current_owner == Some(owner) {
                units.extend(game.presentation.selection.units.iter().copied());
                units.sort_unstable();
                units.dedup();
            }
        }
        game.presentation.selection.units = units;
        game.presentation.selection.buildings.clear();
        return;
    }

    let foreign_building_owner = game
        .state
        .buildings()
        .iter()
        .filter(|building| building.player != game.presentation.human)
        .filter(|building| selectable_building(game, building))
        .filter(building_inside)
        .map(|building| building.player)
        .min();
    if let Some(owner) = foreign_building_owner {
        buildings = game
            .state
            .buildings()
            .iter()
            .filter(|building| building.player == owner && selectable_building(game, building))
            .filter(building_inside)
            .map(|building| building.id)
            .collect();
        if additive {
            let current_owner = game
                .presentation
                .selection
                .buildings
                .first()
                .and_then(|id| game.state.building(*id))
                .map(|building| building.player);
            if current_owner == Some(owner) {
                buildings.extend(game.presentation.selection.buildings.iter().copied());
                buildings.sort_unstable();
                buildings.dedup();
            }
        }
        game.presentation.selection.units.clear();
        game.presentation.selection.buildings = buildings;
        return;
    }

    if !additive {
        game.presentation.selection.units.clear();
        game.presentation.selection.buildings.clear();
        game.presentation.selection.pile = None;
    }
}

/// Double-click: visible entities of the picked kind and owner on screen.
pub(super) fn select_all_of_kind_on_screen(
    game: &mut Game,
    screen: Vec2,
    ui: f32,
    pointer: Pointer,
) {
    let world = game.presentation.camera.to_world(screen);
    // The sweep stays within the picked unit's owner: double-clicking an
    // ally harvester gathers that ally's harvesters on screen. Own units
    // outrank foreign at the pick, as with plain clicks.
    let picked = game
        .state
        .units()
        .iter()
        .filter(|u| selectable(game, u))
        .filter_map(|u| {
            let p = vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
            let distance = p.distance(world);
            (distance <= pick_radius(game, ui, u.kind, pointer)).then_some((
                u.player != game.presentation.human,
                distance,
                u.kind,
                u.player,
            ))
        })
        .min_by(|a, b| (a.0, a.1).partial_cmp(&(b.0, b.1)).expect("finite"));
    let (lo, hi) = game.presentation.camera.world_rect();
    let on_screen = |p: Vec2| p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y;
    if let Some((_, _, kind, owner)) = picked {
        game.presentation.selection.buildings.clear();
        game.presentation.selection.units = game
            .state
            .units()
            .iter()
            .filter(|u| u.player == owner && u.kind == kind && selectable(game, u))
            .filter(|u| on_screen(vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>())))
            .map(|u| u.id)
            .collect();
        return;
    }

    let tile = numeric::tile_at(world);
    let Some(picked) = game
        .state
        .buildings_at(tile)
        .find(|building| selectable_building(game, building))
    else {
        return;
    };
    let (kind, owner) = (picked.kind, picked.player);
    game.presentation.selection.units.clear();
    game.presentation.selection.buildings = game
        .state
        .buildings()
        .iter()
        .filter(|building| {
            building.player == owner && building.kind == kind && selectable_building(game, building)
        })
        .filter(|building| {
            let center = building.center();
            on_screen(vec2(center.x.to_num::<f32>(), center.y.to_num::<f32>()))
        })
        .map(|building| building.id)
        .collect();
}
