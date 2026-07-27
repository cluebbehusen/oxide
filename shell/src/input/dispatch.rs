//! The action dispatcher: every semantic [`Action`] a chord resolves to
//! lands here exactly once, whether it came from a key, a panel card,
//! or an injected event.

use super::InputState;
use super::orders::{digit_action, train};
use super::select::{cycle_idle_worker, idle_harvesters};
use crate::action::Action;
use crate::game::Game;
use macroquad::prelude::{Vec2, vec2};
use oxide_sim::{Command, UnitKind};

pub(super) fn dispatch_action(game: &mut Game, input: &mut InputState, action: Action) {
    match action {
        // Continuous pans live in update_held; Confirm belongs to menus.
        Action::PanLeft | Action::PanRight | Action::PanUp | Action::PanDown => {}
        Action::Confirm => {}
        Action::Slot(n) => digit_action(game, input, (n - 1) as usize),
        Action::AssignGroup(n) => {
            // Groups 1-5, like the recall side; the classic layout never
            // had more. Only own units enter a group — an inspected
            // ally in a control group would dead-lock recalls under
            // own-gating, so foreign picks drop at ASSIGN time.
            let slot = (n - 1) as usize;
            if slot < input.groups.len() {
                let own: Vec<_> = game
                    .selection
                    .units
                    .iter()
                    .copied()
                    .filter(|id| game.state.unit(*id).is_some_and(|u| u.player == game.human))
                    .collect();
                if own.len() < game.selection.units.len() {
                    game.toast("only your own units join a control group");
                }
                input.groups[slot] = own;
            }
        }
        Action::StopOrScrap => {
            // Contextual: units selected halt in place; a selected own
            // unfinished site is scrapped for its refund.
            if !game.selection.units.is_empty() && !game.selection_commandable() {
                game.toast("ally units are read-only");
                return;
            }
            if !game.selection.units.is_empty() {
                let units = game.selection.units.clone();
                game.issue(Command::Stop { units });
            } else if let Some(id) = game.selection.building
                && game
                    .state
                    .building(id)
                    .is_some_and(|b| b.player == game.human && !b.built)
            {
                game.issue(Command::Cancel { building: id });
                game.selection.building = None;
            }
        }
        Action::TrainSlot(n) => train(game, n as usize),
        Action::TogglePause => game.paused = !game.paused,
        Action::ToggleBuildPalette => {
            if input.build_menu {
                input.build_menu = false;
                return;
            }
            let has_builder = game.selection.units.iter().any(|id| {
                game.state
                    .unit(*id)
                    .is_some_and(|u| u.kind == UnitKind::Harvester && u.player == game.human)
            });
            if has_builder {
                input.build_menu = true;
                input.placing = None;
            } else {
                // No builder in hand — the key still means "I want to
                // build": grab the nearest own harvester (idle ones
                // first), select it, and open the palette. The camera
                // stays put; the machine walks to wherever the player
                // places.
                let idle = idle_harvesters(game);
                let cx = game.camera.center.x.floor() as i32;
                let cy = game.camera.center.y.floor() as i32;
                let pick = game
                    .state
                    .units()
                    .iter()
                    .filter(|u| u.player == game.human && u.kind == UnitKind::Harvester)
                    .filter(|u| idle.is_empty() || idle.contains(&u.id))
                    .min_by_key(|u| {
                        let t = u.tile();
                        let (dx, dy) = (i64::from(t.x - cx), i64::from(t.y - cy));
                        (dx * dx + dy * dy, u.id.0)
                    })
                    .map(|u| u.id);
                if let Some(id) = pick {
                    game.selection.units = vec![id];
                    game.selection.building = None;
                    input.build_menu = true;
                    input.placing = None;
                } else {
                    game.toast("no harvester to build with");
                }
            }
        }
        Action::Patrol => {
            if !game.selection_commandable() {
                game.toast("ally units are read-only");
                return;
            }
            // First press arms a route; the second sends the circuit.
            match input.patrol_route.take() {
                None if !game.selection.units.is_empty() => {
                    input.patrol_route = Some(Vec::new());
                    game.toast("patrol: right-click waypoints, R to start");
                }
                None => {}
                Some(route) if route.is_empty() => {
                    game.toast("patrol cancelled");
                }
                Some(waypoints) => {
                    let units = game.selection.units.clone();
                    game.issue(Command::Patrol { units, waypoints });
                }
            }
        }
        Action::ToggleOverlay => game.overlay = !game.overlay,
        Action::Back => {
            // Arming something? Escape abandons that first.
            if input.build_menu {
                input.build_menu = false;
                return;
            }
            if input.placing.take().is_some() {
                game.toast("placement cancelled");
                return;
            }
            if input.salvaging {
                input.salvaging = false;
                game.toast("salvage cancelled");
                return;
            }
            if input.running {
                input.running = false;
                game.toast("run cancelled");
                return;
            }
            if input.patrol_route.take().is_some() {
                game.toast("patrol cancelled");
                return;
            }
            game.selection.units.clear();
            game.selection.building = None;
        }
        Action::SetBookmark(slot) => {
            input.bookmarks[slot as usize] = Some(game.camera.center);
            game.toast(format!("bookmark {} set", slot + 1));
        }
        Action::RecallBookmark(slot) => {
            if let Some(center) = input.bookmarks[slot as usize] {
                game.camera.center = center;
                game.camera.pan(Vec2::ZERO); // re-clamp
            }
        }
        Action::Salvage => {
            // A toggle, like the palette: pressing again stands down.
            if input.salvaging {
                input.salvaging = false;
                game.toast("salvage cancelled");
                return;
            }
            let has_harvester = game.selection.units.iter().any(|id| {
                game.state
                    .unit(*id)
                    .is_some_and(|u| u.kind == UnitKind::Harvester && u.player == game.human)
            });
            if has_harvester {
                input.salvaging = true;
                game.toast("salvage: click an own building to strip it, Esc to cancel");
            } else {
                game.toast("no harvester to salvage with");
            }
        }
        Action::Run => {
            // A toggle, like salvage: pressing again stands down.
            if input.running {
                input.running = false;
                game.toast("run cancelled");
                return;
            }
            let has_own_unit = game
                .selection
                .units
                .iter()
                .any(|id| game.state.unit(*id).is_some_and(|u| u.player == game.human));
            if has_own_unit {
                input.running = true;
                game.toast("run: click ground to move without engaging, Esc to cancel");
            } else {
                game.toast("no machines selected to run");
            }
        }
        Action::CycleIdleWorker => cycle_idle_worker(game),
        Action::JumpToLastAlert => {
            if let Some(world) = game.last_alert {
                game.camera.center = world;
                game.camera.pan(Vec2::ZERO); // re-clamp
            } else {
                game.toast("no recent alerts");
            }
        }
        Action::HomeCamera => {
            if let Some(center) = game.home_foundry().map(|b| b.center()) {
                let target = vec2(center.x.to_num::<f32>(), center.y.to_num::<f32>());
                game.camera.center = target;
                game.camera.pan(vec2(0.0, 0.0)); // re-clamp
            }
        }
    }
}
