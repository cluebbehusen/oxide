//! The action dispatcher: every semantic [`Action`] a chord resolves to
//! lands here exactly once, whether it came from a key, a panel card,
//! or an injected event.

use super::orders::digit_action;
use super::select::{cycle_idle_worker, idle_harvesters};
use super::{InputState, armed_toast};
use crate::action::Action;
use crate::game::Game;
use macroquad::prelude::{Vec2, vec2};
use oxide_sim::Command;

pub(super) fn dispatch_action(game: &mut Game, input: &mut InputState, action: Action) {
    if input.construction_open()
        && matches!(
            action,
            Action::Run
                | Action::AttackMove
                | Action::Salvage
                | Action::RepairUnit
                | Action::Unload
                | Action::ReturnCargo
        )
    {
        input.close_construction();
    }
    match action {
        // Continuous pans live in update_held; Confirm belongs to menus.
        Action::PanLeft | Action::PanRight | Action::PanUp | Action::PanDown => {}
        Action::Confirm
        | Action::ReplayPause
        | Action::ReplayBack
        | Action::ReplayForward
        | Action::ReplayStart
        | Action::ReplayEnd
        | Action::ReplaySpeed(_)
        | Action::ReplayStats
        | Action::MenuUp
        | Action::MenuDown
        | Action::MenuLeft
        | Action::MenuRight
        | Action::MenuPageUp
        | Action::MenuPageDown
        | Action::MenuHome
        | Action::MenuEnd
        | Action::DeleteSave => {}
        Action::Slot(n) => digit_action(game, input, (n - 1) as usize),
        Action::AssignGroup(n) => {
            // Groups 1-5, like the recall side; the classic layout never
            // had more. Only own units enter a group — an inspected
            // ally in a control group would dead-lock recalls under
            // own-gating, so foreign picks drop at ASSIGN time.
            let slot = (n - 1) as usize;
            if slot < input.groups.len() {
                let own: Vec<_> = game
                    .presentation
                    .selection
                    .units
                    .iter()
                    .copied()
                    .filter(|id| {
                        game.state
                            .unit(*id)
                            .is_some_and(|u| u.player == game.presentation.human)
                    })
                    .collect();
                if own.len() < game.presentation.selection.units.len() {
                    game.presentation
                        .toast("only your own units join a control group");
                }
                input.groups[slot] = own;
            }
        }
        Action::ReturnCargo => {
            if !game.selection_commandable() {
                game.presentation
                    .toast("You can only command your own units.");
                return;
            }
            if !game.presentation.selection.units.iter().any(|id| {
                game.state
                    .unit(*id)
                    .is_some_and(|unit| unit.kind.stats().harvest.is_some() && unit.carrying > 0)
            }) {
                game.presentation.toast("No scrap carried.");
                return;
            }
            input.disarm_click_verbs();
            input.patrol_route = None;
            game.issue(Command::ReturnCargo {
                units: game.presentation.selection.units.clone(),
                foundry: None,
                repair: false,
            });
        }
        Action::StopOrScrap => {
            // Contextual: units selected halt in place; a selected own
            // unfinished site is scrapped for its refund.
            if !game.presentation.selection.units.is_empty() && !game.selection_commandable() {
                game.presentation
                    .toast("You can only command your own units.");
                return;
            }
            if !game.presentation.selection.units.is_empty() {
                let units = game.presentation.selection.units.clone();
                game.issue(Command::Stop { units });
            } else {
                crate::building_actions::stop_or_scrap(game);
            }
        }
        Action::TrainSlot(_)
        | Action::Upgrade
        | Action::SetRally
        | Action::ClearRally
        | Action::Unload
        | Action::Build(_) => super::activate_action_card(game, input, action),
        Action::BuildCategory(category) => {
            input.disarm_click_verbs();
            input.patrol_route = None;
            input.build_menu = true;
            input.build_category = Some(category);
        }
        Action::TogglePause => game.presentation.paused = !game.presentation.paused,
        Action::ToggleBuildPalette => {
            if input.construction_open() {
                if input.build_category.take().is_some() {
                    input.disarm_click_verbs();
                    input.build_menu = true;
                } else {
                    input.close_construction();
                }
                return;
            }
            input.close_construction();
            let has_builder = game.presentation.selection.units.iter().any(|id| {
                game.state.unit(*id).is_some_and(|u| {
                    u.kind.stats().harvest.is_some() && u.player == game.presentation.human
                })
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
                let idle = idle_harvesters(&game.view());
                let cx = game.presentation.camera.center.x.floor() as i32;
                let cy = game.presentation.camera.center.y.floor() as i32;
                let pick = game
                    .state
                    .units()
                    .iter()
                    .filter(|u| {
                        u.player == game.presentation.human && u.kind.stats().harvest.is_some()
                    })
                    .filter(|u| idle.is_empty() || idle.contains(&u.id))
                    .min_by_key(|u| {
                        let t = u.tile();
                        let (dx, dy) = (i64::from(t.x - cx), i64::from(t.y - cy));
                        (dx * dx + dy * dy, u.id.0)
                    })
                    .map(|u| u.id);
                if let Some(id) = pick {
                    game.presentation.selection.units = vec![id];
                    game.presentation.selection.buildings.clear();
                    input.build_menu = true;
                    input.placing = None;
                } else {
                    game.presentation.toast("no harvester to build with");
                }
            }
        }
        Action::Patrol => {
            if !game.selection_commandable() {
                game.presentation
                    .toast("You can only command your own units.");
                return;
            }
            // First press arms a route; the second sends the circuit.
            match input.patrol_route.take() {
                None if !game.presentation.selection.units.is_empty() => {
                    input.patrol_route = Some(Vec::new());
                    game.presentation.toast(format!(
                        "patrol: right-click waypoints, {} to start",
                        input.bindings.label(Action::Patrol)
                    ));
                }
                None => {}
                Some(route) if route.is_empty() => {
                    game.presentation.toast("patrol cancelled");
                }
                Some(waypoints) => {
                    let units = game.presentation.selection.units.clone();
                    game.issue(Command::Patrol { units, waypoints });
                }
            }
        }
        Action::ToggleOverlay => game.presentation.overlay = !game.presentation.overlay,
        Action::Back => {
            // Arming something? Escape abandons that first.
            if input.placing.take().is_some() {
                input.placing_stroke = None;
                input.build_menu = true;
                game.presentation.toast("placement cancelled");
                return;
            }
            if input.build_category.take().is_some() {
                input.build_menu = true;
                return;
            }
            if input.build_menu {
                input.close_construction();
                return;
            }
            if input.salvaging {
                input.salvaging = false;
                game.presentation.toast("salvage cancelled");
                return;
            }
            if input.repairing {
                input.repairing = false;
                game.presentation.toast("weld cancelled");
                return;
            }
            if input.running {
                input.running = false;
                game.presentation.toast("run cancelled");
                return;
            }
            if input.attacking {
                input.attacking = false;
                game.presentation.toast("attack-move cancelled");
                return;
            }
            if !input.rallying.is_empty() {
                input.rallying.clear();
                game.presentation.toast("rally placement cancelled");
                return;
            }
            if input.patrol_route.take().is_some() {
                game.presentation.toast("patrol cancelled");
                return;
            }
            game.presentation.selection.units.clear();
            game.presentation.selection.buildings.clear();
        }
        Action::SetBookmark(slot) => {
            input.bookmarks[slot as usize] = Some(game.presentation.camera.center);
            game.presentation
                .toast(format!("bookmark {} set", slot + 1));
        }
        Action::RecallBookmark(slot) => {
            if let Some(center) = input.bookmarks[slot as usize] {
                game.presentation.camera.center = center;
                game.presentation.camera.pan(Vec2::ZERO); // re-clamp
            }
        }
        Action::Salvage => {
            // A toggle, like the palette: pressing again stands down.
            if input.salvaging {
                input.salvaging = false;
                game.presentation.toast("salvage cancelled");
                return;
            }
            let has_worker = game.presentation.selection.units.iter().any(|id| {
                game.state.unit(*id).is_some_and(|u| {
                    u.kind.stats().harvest.is_some() && u.player == game.presentation.human
                })
            });
            if has_worker {
                input.disarm_click_verbs();
                input.salvaging = true;
                game.presentation.toast(armed_toast(
                    "salvage",
                    "an own building to strip it",
                    &input.bindings.label(Action::Back),
                    crate::platform::TOUCH_ONLY,
                ));
            } else {
                game.presentation.toast("no worker to salvage with");
            }
        }
        Action::RepairUnit => {
            // A toggle, like salvage: pressing again stands down.
            if input.repairing {
                input.repairing = false;
                game.presentation.toast("weld cancelled");
                return;
            }
            let has_welder = game.presentation.selection.units.iter().any(|id| {
                game.state
                    .unit(*id)
                    .is_some_and(|u| u.kind.stats().welder && u.player == game.presentation.human)
            });
            if has_welder {
                input.disarm_click_verbs();
                input.repairing = true;
                game.presentation.toast(armed_toast(
                    "weld",
                    "a damaged own unit",
                    &input.bindings.label(Action::Back),
                    crate::platform::TOUCH_ONLY,
                ));
            } else {
                game.presentation.toast("no welder in hand");
            }
        }
        Action::Run => {
            // A toggle, like salvage: pressing again stands down.
            if input.running {
                input.running = false;
                game.presentation.toast("run cancelled");
                return;
            }
            let has_own_unit = game.presentation.selection.units.iter().any(|id| {
                game.state
                    .unit(*id)
                    .is_some_and(|u| u.player == game.presentation.human)
            });
            if has_own_unit {
                input.disarm_click_verbs();
                input.running = true;
                game.presentation.toast(armed_toast(
                    "run",
                    "ground to move without engaging",
                    &input.bindings.label(Action::Back),
                    crate::platform::TOUCH_ONLY,
                ));
            } else {
                game.presentation.toast("no machines selected to run");
            }
        }
        Action::AttackMove => {
            if input.attacking {
                input.attacking = false;
                game.presentation.toast("attack-move cancelled");
                return;
            }
            let has_own_unit = game.presentation.selection.units.iter().any(|id| {
                game.state
                    .unit(*id)
                    .is_some_and(|u| u.player == game.presentation.human)
            });
            if has_own_unit {
                input.disarm_click_verbs();
                input.attacking = true;
                game.presentation.toast(armed_toast(
                    "attack-move",
                    "ground to engage and chase",
                    &input.bindings.label(Action::Back),
                    crate::platform::TOUCH_ONLY,
                ));
            } else {
                game.presentation
                    .toast("no machines selected to attack-move");
            }
        }
        Action::CycleIdleWorker => cycle_idle_worker(game),
        Action::JumpToLastAlert => {
            if let Some(world) = game.presentation.last_alert {
                game.presentation.camera.center = world;
                game.presentation.camera.pan(Vec2::ZERO); // re-clamp
            } else {
                game.presentation.toast("no recent alerts");
            }
        }
        Action::HomeCamera => {
            if let Some(center) = game.home_foundry().map(|b| b.center()) {
                let target = vec2(center.x.to_num::<f32>(), center.y.to_num::<f32>());
                game.presentation.camera.center = target;
                game.presentation.camera.pan(vec2(0.0, 0.0)); // re-clamp
            }
        }
    }
}
