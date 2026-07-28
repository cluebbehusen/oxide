//! Turning intent into sim commands: the right-click context order,
//! contextual digits, control groups, and factory training. The sim
//! re-validates everything — this module only shapes intent, and its
//! fog checks exist so a click cannot *probe* what the player can't see.

use super::{BUILD_PALETTE, InputState, PICK_RADIUS};
use crate::game::{Game, PingKind};
use chassis::grid::TilePos;
use macroquad::prelude::{Vec2, vec2};
use oxide_sim::{Command, Target, UnitId, UnitKind};

/// Digits are contextual: an open build palette spends them on
/// structures, a selected own factory spends them on production, and
/// otherwise the first five are control groups.
pub(super) fn digit_action(game: &mut Game, input: &mut InputState, slot: usize) {
    if input.build_menu {
        if let Some(&kind) = BUILD_PALETTE.get(slot) {
            input.build_menu = false;
            input.disarm_click_verbs();
            input.placing = Some(kind);
            let cost = kind.stats().construction.map(|c| c.cost).unwrap_or(0);
            game.toast(format!(
                "placing {} ({} scrap): click to build, Esc to cancel",
                kind.name(),
                cost
            ));
        }
        return;
    }
    let producing = game.selection.building.is_some_and(|id| {
        game.state.building(id).is_some_and(|b| {
            b.player == game.human && b.built && !b.kind.stats().produces.is_empty()
        })
    });
    if producing {
        train(game, slot);
        return;
    }
    if slot < 5 {
        group_action(game, input, slot);
    }
}

/// Recall (or with Ctrl, assign) a control group; a quick double-tap on
/// the same slot centers the camera on the group.
fn group_action(game: &mut Game, input: &mut InputState, slot: usize) {
    // Ownership, not mere existence: after a session change a stale id
    // could name anyone's unit (belt to reset_session's suspenders).
    let alive: Vec<UnitId> = input.groups[slot]
        .iter()
        .copied()
        .filter(|id| game.state.unit(*id).is_some_and(|u| u.player == game.human))
        .collect();
    input.groups[slot] = alive.clone();
    if alive.is_empty() {
        return;
    }
    game.selection.units = alive.clone();
    game.selection.building = None;
    let now = input.now;
    if input
        .last_recall
        .is_some_and(|(s, t)| s == slot && now - t < 0.4)
    {
        let mut sum = vec2(0.0, 0.0);
        for id in &alive {
            let u = game.state.unit(*id).expect("pruned above");
            sum += vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
        }
        game.camera.center = sum / alive.len() as f32;
        game.camera.pan(Vec2::ZERO); // re-clamp
    }
    input.last_recall = Some((slot, now));
}

/// Right-click: order the selection by what's under the cursor — enemy →
/// attack, scrap → harvest, ground → move. The sim re-validates everything;
/// this is only intent.
pub(super) fn context_order(game: &mut Game, screen: Vec2, queue: bool) {
    let world = game.camera.to_world(screen);
    let tile = TilePos::new(world.x.floor() as i32, world.y.floor() as i32);
    if game.selection.units.is_empty() {
        // A selected own building takes right-clicks as its rally point.
        if let Some(building) = game.selection.building
            && game
                .state
                .building(building)
                .is_some_and(|b| b.player == game.human)
        {
            game.issue(Command::SetRally {
                building,
                rally: Some(tile),
            });
            game.ping(world, PingKind::Rally);
        }
        return;
    }
    if !game.selection_commandable() {
        game.toast("ally units are read-only");
        return;
    }
    let units = game.selection.units.clone();
    let has_harvester = units.iter().any(|id| {
        game.state
            .unit(*id)
            .is_some_and(|u| u.kind == UnitKind::Harvester)
    });

    // Own-FOOTPRINT hits outrank enemy-RADIUS hits: a raider gnawing a
    // wall sits inside the pick radius of a click on that wall, and the
    // click's plain meaning is the building under the cursor, not the
    // rat beside it. No visibility condition on own targets — ownership
    // cannot probe fog, and own buildings always draw.
    if has_harvester
        && let Some(building) = game.state.building_at(tile)
        && building.player == game.human
    {
        if !building.built {
            // Resume the site: the sim commits every accepted
            // harvester (builders stack). Send the building's own
            // anchor and kind — the cursor may be on any footprint
            // tile of a 2x2.
            game.issue(Command::Build {
                units,
                kind: building.kind,
                anchor: building.anchor,
                queue,
                defer: false,
            });
            game.ping(world, PingKind::Harvest);
            return;
        }
        if building.hp < building.kind.stats().max_hp {
            game.issue(Command::Repair {
                units,
                building: building.id,
                queue,
            });
            game.ping(world, PingKind::Harvest);
            return;
        }
        // A healthy built own building: fall through (ground order).
    }

    // Fog rules what right-click may target: unseen enemies aren't there
    // as far as the player is concerned (the sim enforces this too).
    let enemy_unit = game
        .state
        .units()
        .iter()
        .filter(|u| game.state.hostile(game.human, u.player) && game.my_vision().visible(u.tile()))
        .map(|u| {
            let p = vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
            (p.distance(world), u.id)
        })
        .filter(|(d, _)| *d <= PICK_RADIUS)
        .min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((_, target)) = enemy_unit {
        let at = game
            .state
            .unit(target)
            .map(|u| vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>()))
            .unwrap_or(world);
        game.issue(Command::Attack {
            units,
            target: Target::Unit(target),
            queue,
        });
        game.ping(at, PingKind::Attack);
        return;
    }
    if let Some(building) = game.state.building_at(tile)
        && game.state.hostile(game.human, building.player)
        && building.tiles().any(|t| game.my_vision().visible(t))
    {
        let target = Target::Building(building.id);
        game.issue(Command::Attack {
            units,
            target,
            queue,
        });
        game.ping(world, PingKind::Attack);
        return;
    }
    // A wounded own GROUND unit under the cursor takes the weld, the
    // unit mirror of the damaged-building flow above — but only AFTER
    // the enemy checks (attack intent stays reliable in a brawl) and
    // never for a machine in the current selection, so ordering a
    // group that contains its own wounded still reads as a move. The
    // armed verb (the Weld card) reaches those.
    if has_harvester {
        let patient = game
            .state
            .units()
            .iter()
            .filter(|u| {
                u.player == game.human
                    && u.hp > 0
                    && u.hp < u.kind.stats().max_hp
                    && u.kind.stats().domain == oxide_sim::stats::Domain::Ground
                    && !game.selection.units.contains(&u.id)
            })
            .map(|u| {
                let p = vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
                (p.distance(world), u.id)
            })
            .filter(|(d, _)| *d <= PICK_RADIUS)
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, target)) = patient {
            game.issue(Command::RepairUnit {
                units,
                target,
                queue,
            });
            game.ping(world, PingKind::Harvest);
            return;
        }
    }
    // The harvest check reads the player's *memory*, not the live map —
    // probing fog with right-clicks must not reveal hidden scrap. Wreck
    // memory counts the same as node memory.
    if (game.my_vision().remembered_scrap(tile) > 0 || game.my_vision().remembered_wreck(tile) > 0)
        && has_harvester
    {
        game.issue(Command::Harvest {
            units,
            node: tile,
            queue,
        });
        game.ping(world, PingKind::Harvest);
        return;
    }
    // Fire at will: ground orders engage whatever shows up on the way.
    // Combat units attack-move; the sim degrades harvesters to a plain
    // walk. There is no hold-fire stance (yet — nothing to hide from).
    game.issue(Command::AttackMove {
        units,
        goal: tile,
        queue,
    });
    game.ping(world, PingKind::Move);
}

/// Train the selected factory's Nth product (the seat's own roster —
/// the other faction's variants are skipped). `H`/`S` alias the first
/// two slots; no factory selected falls back to the home Foundry.
pub(super) fn train(game: &mut Game, slot: usize) {
    let building = game
        .selection
        .building
        .filter(|id| {
            game.state
                .building(*id)
                .is_some_and(|b| b.player == game.human)
        })
        .or_else(|| game.home_foundry().map(|b| b.id));
    if let Some(building) = building {
        let faction = game.state.player(game.human).faction;
        let Some(&kind) = game.state.building(building).and_then(|b| {
            b.kind
                .stats()
                .produces
                .iter()
                .filter(|k| k.faction().is_none_or(|f| f == faction))
                .nth(slot)
        }) else {
            return;
        };
        game.issue(Command::Train { building, kind });
    }
}
