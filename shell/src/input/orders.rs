//! Turning intent into sim commands: the right-click context order,
//! contextual digits, control groups, and factory training. The sim
//! re-validates everything — this module only shapes intent, and its
//! fog checks exist so a click cannot *probe* what the player can't see.

use super::{InputState, PICK_RADIUS};
use crate::game::{Game, PingKind};
use chassis::grid::TilePos;
use macroquad::prelude::{Vec2, vec2};
use oxide_sim::{Command, Target, UnitId};

pub(super) fn selected_producers(game: &Game) -> Vec<oxide_sim::BuildingId> {
    game.selection
        .buildings
        .iter()
        .copied()
        .filter(|id| {
            game.state.building(*id).is_some_and(|building| {
                building.player == game.human
                    && building.built
                    && !building.stats().produces.is_empty()
            })
        })
        .collect()
}

pub(super) fn rally_selected_producers(game: &mut Game, rally: TilePos, at: Vec2) {
    let producers = selected_producers(game);
    if producers.is_empty() {
        return;
    }
    for building in producers {
        game.issue(Command::SetRally {
            building,
            rally: Some(rally),
        });
    }
    game.ping(at, PingKind::Rally);
}

fn visible_hostile_target_at(
    game: &Game,
    world: Vec2,
    tile: TilePos,
) -> Option<(Target, Vec2, oxide_sim::stats::Domain)> {
    let unit = game
        .state
        .units()
        .iter()
        .filter(|unit| {
            game.state.hostile(game.human, unit.player) && game.my_vision().visible(unit.tile())
        })
        .map(|unit| {
            let position = vec2(unit.pos.x.to_num::<f32>(), unit.pos.y.to_num::<f32>());
            (
                position.distance(world),
                unit.id,
                position,
                // Its layer right now: a parked airframe is a ground
                // target for the weapons that can reach ground.
                unit.domain(),
            )
        })
        .filter(|(distance, ..)| *distance <= PICK_RADIUS)
        .min_by(|left, right| {
            left.0
                .total_cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
        });
    if let Some((_, id, position, domain)) = unit {
        return Some((Target::Unit(id), position, domain));
    }
    game.state
        .building_at(tile)
        .filter(|building| {
            game.state.hostile(game.human, building.player)
                && building
                    .tiles()
                    .any(|footprint| game.my_vision().visible(footprint))
                && game.state.building_apparent(game.human, building)
        })
        .map(|building| {
            (
                Target::Building(building.id),
                world,
                oxide_sim::stats::Domain::Ground,
            )
        })
}

/// Digits are contextual: an open build palette spends them on
/// structures, a selected own factory spends them on production, and
/// otherwise the first five are control groups.
pub(super) fn digit_action(game: &mut Game, input: &mut InputState, slot: usize) {
    if input.build_menu {
        if let Some(&kind) = crate::input::build_page(input.build_page).get(slot) {
            input.build_menu = false;
            input.disarm_click_verbs();
            input.placing = Some(kind);
            let cost = kind.base_stats().construction.map(|c| c.cost).unwrap_or(0);
            game.toast(format!(
                "placing {} ({} scrap): click to build, Esc to cancel",
                kind.name(),
                cost
            ));
        }
        return;
    }
    let producing = !selected_producers(game).is_empty();
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
    game.selection.buildings.clear();
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
/// attack, scrap → harvest, ground → advance. The sim re-validates everything;
/// this is only intent.
pub(super) fn context_order(game: &mut Game, screen: Vec2, queue: bool) {
    let world = game.camera.to_world(screen);
    let tile = TilePos::new(world.x.floor() as i32, world.y.floor() as i32);
    if game.selection.units.is_empty() {
        if let Some((target, at, domain)) = visible_hostile_target_at(game, world, tile) {
            let defenses: Vec<_> = game
                .selection
                .buildings
                .iter()
                .copied()
                .filter(|id| {
                    game.state.building(*id).is_some_and(|building| {
                        building.player == game.human
                            && building.built
                            && building
                                .kind
                                .base_stats()
                                .weapons
                                .first()
                                .is_some_and(|weapon| weapon.targets.covers(domain))
                    })
                })
                .collect();
            if !defenses.is_empty() {
                game.issue(Command::FocusFire {
                    buildings: defenses,
                    target,
                });
                game.ping(at, PingKind::Attack);
                return;
            }
        }
        // Selected producers share one rally destination. Non-producers
        // ignore a ground right-click.
        rally_selected_producers(game, tile, world);
        return;
    }
    if !game.selection_commandable() {
        game.toast("ally units are read-only");
        return;
    }
    let units = game.selection.units.clone();
    // Each verb crews by its own capability, mirroring the sim's crew
    // filters exactly: workers (harvest kit) carry build and harvest
    // labor, welders carry the torch. A coarser union here would stage
    // commands the sim rejects with an empty crew.
    let has_worker = units.iter().any(|id| {
        game.state
            .unit(*id)
            .is_some_and(|u| u.kind.stats().harvest.is_some())
    });
    let has_welder = units
        .iter()
        .any(|id| game.state.unit(*id).is_some_and(|u| u.kind.stats().welder));

    // Own-FOOTPRINT hits outrank enemy-RADIUS hits: a raider gnawing a
    // wall sits inside the pick radius of a click on that wall, and the
    // click's plain meaning is the building under the cursor, not the
    // rat beside it. No visibility condition on own targets — ownership
    // cannot probe fog, and own buildings always draw.
    if (has_worker || has_welder)
        && let Some(building) = game.state.building_at(tile)
        && building.player == game.human
    {
        if !building.built {
            if building.tier > 0 {
                game.toast("upgrade runs automatically");
                return;
            }
            if has_worker {
                // Resume the site: the sim commits every accepted
                // worker (builders stack). Send the building's own
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
            // Welders alone cannot lay construction: fall through.
        } else if building.hp < building.stats().max_hp && has_welder {
            game.issue(Command::Repair {
                units,
                building: building.id,
                queue,
            });
            game.ping(world, PingKind::Harvest);
            return;
        }
        // A healthy built own building — or a site without a worker, or
        // a wound without a torch — falls through to the ground order.
    }

    // Carriable ground machines right-clicked onto an own transport
    // climb aboard — before the hostile check reads the ground under
    // the hovering airframe.
    let carriable: Vec<oxide_sim::UnitId> = units
        .iter()
        .copied()
        .filter(|id| {
            game.state
                .unit(*id)
                .is_some_and(|u| u.kind.stats().transport_size > 0)
        })
        .collect();
    if !carriable.is_empty() {
        let sling = game
            .state
            .units()
            .iter()
            .filter(|u| {
                u.player == game.human
                    && u.hp > 0
                    && u.kind.stats().transport_capacity > 0
                    && !game.selection.units.contains(&u.id)
            })
            .map(|u| {
                let p = vec2(u.pos.x.to_num::<f32>(), u.pos.y.to_num::<f32>());
                (p.distance(world), u.id)
            })
            .filter(|(d, _)| *d <= PICK_RADIUS)
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, transport)) = sling {
            game.issue(Command::Load {
                units: carriable,
                transport,
                queue,
            });
            game.ping(world, PingKind::Move);
            return;
        }
    }
    // Fog rules what right-click may target: unseen enemies aren't there
    // as far as the player is concerned (the sim enforces this too).
    if let Some((target, at, _)) = visible_hostile_target_at(game, world, tile) {
        game.issue(Command::Attack {
            units,
            target,
            queue,
        });
        game.ping(at, PingKind::Attack);
        return;
    }
    // A wounded own GROUND unit under the cursor takes the weld, the
    // unit mirror of the damaged-building flow above — but only AFTER
    // the enemy checks (attack intent stays reliable in a brawl) and
    // never for a machine in the current selection, so ordering a
    // group that contains its own wounded still reads as a move. The
    // armed verb (the Weld card) reaches those.
    if has_welder {
        let patient = game
            .state
            .units()
            .iter()
            .filter(|u| {
                u.player == game.human
                    && u.hp > 0
                    && u.hp < u.kind.stats().max_hp
                    && u.domain() == oxide_sim::stats::Domain::Ground
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
        && has_worker
    {
        game.issue(Command::Harvest {
            units,
            node: tile,
            queue,
        });
        game.ping(world, PingKind::Harvest);
        return;
    }
    // Default ground movement keeps formation intent: weapons take
    // already-available shots, but machines never stop or chase.
    // Explicit attack-move is the F verb.
    game.issue(Command::Advance {
        units,
        goal: tile,
        queue,
    });
    game.ping(world, PingKind::Move);
}

/// Train the Nth product from the first compatible selected producer (the
/// seat's own roster — the other faction's variants are skipped). `H`/`S`
/// alias the first two slots; no producer selected falls back to the home
/// Foundry.
pub(super) fn train(game: &mut Game, slot: usize) {
    let faction = game.state.player(game.human).faction;
    let product = |building| {
        game.state.building(building).and_then(|building| {
            building
                .kind
                .base_stats()
                .produces
                .iter()
                .filter(|k| k.faction().is_none_or(|f| f == faction))
                .nth(slot)
                .copied()
        })
    };
    let selected = selected_producers(game);
    let selected_choice = selected
        .iter()
        .find_map(|building| product(*building).map(|kind| (*building, kind)));
    let choice = if selected.is_empty() {
        game.home_foundry()
            .and_then(|building| product(building.id).map(|kind| (building.id, kind)))
    } else {
        selected_choice
    };
    if let Some((building, kind)) = choice {
        game.issue(Command::Train { building, kind });
    }
}
