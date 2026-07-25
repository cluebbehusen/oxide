//! Picking and selection: click, box, double-click-kind, and the idle
//! harvester cycle. Since 0.11 any owner's VISIBLE units are
//! selectable — allies for reading, enemies for inspection — but a
//! selection is single-allegiance by construction: picks and merges of
//! a different owner REPLACE it (a mixed own+ally box would dead-lock
//! every command under own-gating). Fog and ownership rules for
//! *orders* live in `orders`.

use crate::game::Game;
use chassis::grid::TilePos;
use macroquad::prelude::{Vec2, vec2};
use oxide_sim::{UnitId, UnitKind};

/// Own harvesters with nothing to do, in id order — the cycle key and
/// the HUD badge both read this.
pub fn idle_harvesters(game: &Game) -> Vec<UnitId> {
    game.state
        .units()
        .iter()
        .filter(|u| {
            u.player == game.human
                && u.kind == UnitKind::Harvester
                && u.order == oxide_sim::Order::Idle
        })
        .map(|u| u.id)
        .collect()
}

/// Selects the next idle harvester after the current selection (id
/// order, wrapping) and centers the camera on it. Stateless: the
/// selection itself is the cursor.
pub(super) fn cycle_idle_worker(game: &mut Game) {
    let idle = idle_harvesters(game);
    let Some(&first) = idle.first() else {
        game.toast("no idle harvesters");
        return;
    };
    let next = match game.selection.units.as_slice() {
        [current] => idle
            .iter()
            .copied()
            .find(|id| id > current)
            .unwrap_or(first),
        _ => first,
    };
    game.selection.units = vec![next];
    game.selection.building = None;
    let unit = game.state.unit(next).expect("listed above");
    game.camera.center = vec2(unit.pos.x.to_num::<f32>(), unit.pos.y.to_num::<f32>());
    game.camera.pan(Vec2::ZERO); // re-clamp
}

/// World-space pick radius around a unit: generous when zoomed out so
/// units never need tweezers (at least 10 logical px on screen).
fn pick_radius(game: &Game, ui: f32) -> f32 {
    (10.0 * ui / game.camera.zoom).max(0.6)
}

/// HUD chrome that swallows clicks: the top bar always; the bottom panel
/// only while it is actually shown — and as tall as it actually drew
/// (the packed palette wraps to several rows on narrow windows; clicks
/// on the upper rows must not fall through to the world).
pub(super) fn click_on_hud(game: &mut Game, screen: Vec2) -> bool {
    game.layout.get().chrome_owns(screen)
}

/// Whether the human may SEE this unit at all — own and allies always
/// (team sight), enemies only on currently visible ground. Selection
/// must never reach through fog.
fn selectable(game: &Game, unit: &oxide_sim::Unit) -> bool {
    !game.state.hostile(game.human, unit.player)
        || game.all_seeing()
        || game.my_vision().visible(unit.tile())
}

pub(super) fn click_select(game: &mut Game, screen: Vec2, additive: bool, ui: f32) {
    let world = game.camera.to_world(screen);
    if !additive {
        game.selection.building = None;
    }
    // Nearest visible unit of any owner within pick range wins; own
    // units outrank foreign ones inside the radius so a scrum never
    // steals the click from the machine you can actually command.
    let radius = pick_radius(game, ui);
    let picked = game
        .state
        .units()
        .iter()
        .filter(|u| selectable(game, u))
        .map(|u| {
            let p = vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
            (u.player != game.human, p.distance(world), u.id, u.player)
        })
        .filter(|(_, d, ..)| *d <= radius)
        .min_by(|a, b| (a.0, a.1).partial_cmp(&(b.0, b.1)).expect("finite"));
    if let Some((_, _, id, owner)) = picked {
        let current_owner = game
            .selection
            .units
            .first()
            .and_then(|u| game.state.unit(*u))
            .map(|u| u.player);
        if additive && current_owner == Some(owner) {
            // Shift-click toggles membership within one allegiance.
            if let Some(index) = game.selection.units.iter().position(|u| *u == id) {
                game.selection.units.remove(index);
            } else {
                game.selection.units.push(id);
            }
        } else {
            // A different owner REPLACES: single-allegiance by
            // construction.
            game.selection.units = vec![id];
        }
        return;
    }
    if additive {
        return; // shift-miss leaves the selection alone
    }
    // …then a building under the cursor (any owner whose ground shows)…
    let tile = TilePos::new(world.x.floor() as i32, world.y.floor() as i32);
    if let Some(building) = game.state.building_at(tile)
        && (!game.state.hostile(game.human, building.player)
            || game.all_seeing()
            || building.tiles().any(|t| game.my_vision().visible(t)))
    {
        game.selection.units.clear();
        game.selection.building = Some(building.id);
        return;
    }
    // …otherwise clear.
    game.selection.units.clear();
}

pub(super) fn box_select(game: &mut Game, a_screen: Vec2, b_screen: Vec2, additive: bool) {
    let a = game.camera.to_world(a_screen);
    let b = game.camera.to_world(b_screen);
    let (lo, hi) = (a.min(b), a.max(b));
    game.selection.building = None;
    let inside = |u: &&oxide_sim::Unit| {
        let p = vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
        p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y
    };
    // Own units in the box always win; a box holding none falls to a
    // single foreign owner (lowest seat first) for inspection — never
    // a mixed bag.
    let mut boxed: Vec<UnitId> = game
        .state
        .units()
        .iter()
        .filter(|u| u.player == game.human)
        .filter(inside)
        .map(|u| u.id)
        .collect();
    if boxed.is_empty() {
        let foreign_owner = game
            .state
            .units()
            .iter()
            .filter(|u| selectable(game, u))
            .filter(inside)
            .map(|u| u.player)
            .min();
        if let Some(owner) = foreign_owner {
            game.selection.units = game
                .state
                .units()
                .iter()
                .filter(|u| u.player == owner)
                .filter(inside)
                .map(|u| u.id)
                .collect();
            return;
        }
    }
    if additive {
        // Merging keeps one allegiance: a foreign remainder in the
        // selection is dropped the moment own units join.
        boxed.extend(
            game.selection
                .units
                .iter()
                .copied()
                .filter(|id| game.state.unit(*id).is_some_and(|u| u.player == game.human)),
        );
        boxed.sort_unstable();
        boxed.dedup();
    }
    game.selection.units = boxed;
}

/// Double-click: everyone of the clicked unit's kind currently on screen.
pub(super) fn select_all_of_kind_on_screen(game: &mut Game, screen: Vec2, ui: f32) {
    let world = game.camera.to_world(screen);
    let radius = pick_radius(game, ui);
    // The sweep stays within the PICKED unit's owner: double-clicking
    // an ally harvester gathers that ally's harvesters on screen, never
    // a cross-allegiance soup. Own units outrank foreign at the pick,
    // like plain clicks.
    let picked = game
        .state
        .units()
        .iter()
        .filter(|u| selectable(game, u))
        .map(|u| {
            let p = vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
            (u.player != game.human, p.distance(world), u.kind, u.player)
        })
        .filter(|(_, d, ..)| *d <= radius)
        .min_by(|a, b| (a.0, a.1).partial_cmp(&(b.0, b.1)).expect("finite"));
    let Some((_, _, kind, owner)) = picked else {
        return;
    };
    let (lo, hi) = game.camera.world_rect();
    game.selection.building = None;
    game.selection.units = game
        .state
        .units()
        .iter()
        .filter(|u| u.player == owner && u.kind == kind && selectable(game, u))
        .filter(|u| {
            let p = vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
            p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y
        })
        .map(|u| u.id)
        .collect();
}
