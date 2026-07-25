//! Picking and selection: click, box, double-click-kind, and the idle
//! harvester cycle. All read the human seat only — fog and ownership
//! rules for *orders* live in `orders`.

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

pub(super) fn click_select(game: &mut Game, screen: Vec2, additive: bool, ui: f32) {
    let world = game.camera.to_world(screen);
    if !additive {
        game.selection.building = None;
    }
    // Nearest own unit within pick range wins…
    let radius = pick_radius(game, ui);
    let picked = game
        .state
        .units()
        .iter()
        .filter(|u| u.player == game.human)
        .map(|u| {
            let p = vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
            (p.distance(world), u.id)
        })
        .filter(|(d, _)| *d <= radius)
        .min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((_, id)) = picked {
        if additive {
            // Shift-click toggles membership.
            if let Some(index) = game.selection.units.iter().position(|u| *u == id) {
                game.selection.units.remove(index);
            } else {
                game.selection.units.push(id);
            }
        } else {
            game.selection.units = vec![id];
        }
        return;
    }
    if additive {
        return; // shift-miss leaves the selection alone
    }
    // …then an own building under the cursor…
    let tile = TilePos::new(world.x.floor() as i32, world.y.floor() as i32);
    if let Some(building) = game.state.building_at(tile)
        && building.player == game.human
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
    let mut boxed: Vec<UnitId> = game
        .state
        .units()
        .iter()
        .filter(|u| u.player == game.human)
        .filter(|u| {
            let p = vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
            p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y
        })
        .map(|u| u.id)
        .collect();
    if additive {
        boxed.extend(game.selection.units.iter().copied());
        boxed.sort_unstable();
        boxed.dedup();
    }
    game.selection.units = boxed;
}

/// Double-click: everyone of the clicked unit's kind currently on screen.
pub(super) fn select_all_of_kind_on_screen(game: &mut Game, screen: Vec2, ui: f32) {
    let world = game.camera.to_world(screen);
    let radius = pick_radius(game, ui);
    let kind = game
        .state
        .units()
        .iter()
        .filter(|u| u.player == game.human)
        .map(|u| {
            let p = vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
            (p.distance(world), u.kind)
        })
        .filter(|(d, _)| *d <= radius)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, k)| k);
    let Some(kind) = kind else {
        return;
    };
    let (lo, hi) = game.camera.world_rect();
    game.selection.building = None;
    game.selection.units = game
        .state
        .units()
        .iter()
        .filter(|u| u.player == game.human && u.kind == kind)
        .filter(|u| {
            let p = vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
            p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y
        })
        .map(|u| u.id)
        .collect();
}
