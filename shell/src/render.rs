//! Drawing: map, entities, effects, HUD, debug overlay.
//!
//! Reads the sim, never writes it. Unit positions interpolate between the
//! previous and current tick so 20 sim ticks per second still looks like
//! 60fps motion.

use crate::assets::Sprites;
use crate::game::{EffectKind, Game};
use crate::input::InputState;
use chassis::grid::TilePos;
use macroquad::prelude::*;
use oxide_sim::stats::SCRAP_NODE_AMOUNT;
use oxide_sim::{GameResult, UnitKind};

const OUTSIDE: Color = color_u8!(20, 20, 25, 255);
const BONE: Color = color_u8!(232, 228, 216, 255);
const BONE_FAINT: Color = color_u8!(232, 228, 216, 90);
const SCRAP_COLOR: Color = color_u8!(217, 164, 65, 255);
const HP_BACK: Color = color_u8!(20, 20, 24, 220);
const DANGER: Color = color_u8!(217, 82, 74, 255);
const PANEL: Color = color_u8!(20, 20, 24, 200);

/// UI scale factor: chrome (text, bars, minimap) is authored in logical
/// pixels and multiplied by this so it reads the same on every display.
pub fn ui_scale() -> f32 {
    macroquad::miniquad::window::dpi_scale().max(1.0)
}

/// Draws one frame.
pub fn draw(game: &Game, sprites: &Sprites, input: &InputState) {
    clear_background(OUTSIDE);
    let alpha = game.render_alpha();
    draw_tiles(game, sprites);
    draw_scorches(game, sprites);
    draw_buildings(game, sprites);
    draw_units(game, sprites, alpha);
    draw_fx(game, sprites);
    // The debug overlay is deliberately omniscient; fog only draws without it.
    if game.overlay {
        draw_overlay(game, alpha);
    } else {
        draw_fog(game);
    }
    // Own-order acknowledgments, rally flags, and radar blips sit above
    // the fog: they are the player's intent and intel, not world state.
    draw_pings(game);
    draw_blips(game);
    draw_rally_marker(game);
    draw_breadcrumbs(game, input);
    draw_placement_ghost(game, sprites, input);
    draw_drag_rect(game, input);
    draw_hud(game, input);
    draw_minimap(game);
}

/// The armed building follows the cursor as a translucent footprint,
/// green-lit where the sim would accept it — the tint and the command
/// share `State::can_place`, so what looks legal is legal.
fn draw_placement_ghost(game: &Game, sprites: &Sprites, input: &InputState) {
    let Some(kind) = input.placing else { return };
    let world = game.camera.to_world(input.mouse);
    let anchor = TilePos::new(world.x.floor() as i32, world.y.floor() as i32);
    let zoom = game.camera.zoom;
    let (w, h) = kind.stats().size;
    let ok = game.state.can_place(game.human, kind, anchor);
    let screen = game
        .camera
        .to_screen(vec2(anchor.x as f32, anchor.y as f32));
    let dest = vec2(w as f32 * zoom, h as f32 * zoom);
    let faction = game.state.player(game.human).faction;
    let tint = if ok {
        Color::new(0.7, 1.0, 0.75, 0.55)
    } else {
        Color::new(1.0, 0.45, 0.4, 0.55)
    };
    draw_texture_ex(
        sprites.texture(),
        screen.x,
        screen.y,
        tint,
        DrawTextureParams {
            dest_size: Some(dest),
            source: Some(sprites.building(kind, faction)),
            ..Default::default()
        },
    );
}

/// Queued waypoints of the selection, drawn as a faint chain; a patrol
/// closes the loop. While arming a patrol (`R`), the collected route
/// draws in scrap-amber instead.
fn draw_breadcrumbs(game: &Game, input: &InputState) {
    let dot = |p: Vec2, color: Color| draw_circle(p.x, p.y, 3.0, color);
    if let Some(route) = &input.patrol_route {
        let mut prev: Option<Vec2> = None;
        for tile in route {
            let p = game
                .camera
                .to_screen(vec2(tile.x as f32 + 0.5, tile.y as f32 + 0.5));
            if let Some(a) = prev {
                draw_line(a.x, a.y, p.x, p.y, 1.5, SCRAP_COLOR);
            }
            dot(p, SCRAP_COLOR);
            prev = Some(p);
        }
        return;
    }
    for id in &game.selection.units {
        let Some(unit) = game.state.unit(*id) else {
            continue;
        };
        // Only explored targets draw: the harvest brain can retarget to a
        // node the player has never seen, and a breadcrumb there would
        // leak it through the fog.
        let goal_of = |order: &oxide_sim::Order| {
            let goal = match order {
                oxide_sim::Order::Move { goal } | oxide_sim::Order::AttackMove { goal } => *goal,
                oxide_sim::Order::Harvest { node } => *node,
                _ => return None,
            };
            (game.overlay || game.my_vision().explored(goal)).then_some(goal)
        };
        let mut points: Vec<Vec2> = Vec::new();
        if let Some(g) = goal_of(&unit.order) {
            points.push(
                game.camera
                    .to_screen(vec2(g.x as f32 + 0.5, g.y as f32 + 0.5)),
            );
        }
        for order in &unit.queue {
            if let Some(g) = goal_of(order) {
                points.push(
                    game.camera
                        .to_screen(vec2(g.x as f32 + 0.5, g.y as f32 + 0.5)),
                );
            }
        }
        if points.is_empty() {
            continue;
        }
        let start = game
            .camera
            .to_screen(vec2(unit.pos.x.to_num::<f32>(), unit.pos.y.to_num::<f32>()));
        let mut prev = start;
        for p in &points {
            draw_line(prev.x, prev.y, p.x, p.y, 1.0, BONE_FAINT);
            dot(*p, BONE_FAINT);
            prev = *p;
        }
        // A patrol is a circuit: close it.
        if unit.looping && points.len() > 1 {
            let first = points[0];
            draw_line(prev.x, prev.y, first.x, first.y, 1.0, BONE_FAINT);
        }
    }
}

const FOG_UNEXPLORED: Color = color_u8!(13, 13, 17, 255);
const FOG_EXPLORED: Color = color_u8!(22, 28, 44, 135);

/// Fog of war from the local player's perspective: unexplored is void,
/// explored-but-unseen is dimmed.
fn draw_fog(game: &Game) {
    let vision = game.my_vision();
    let (min, max) = visible_tiles(game);
    for y in min.y..max.y {
        for x in min.x..max.x {
            let tile = TilePos::new(x, y);
            let cover = if !vision.explored(tile) {
                FOG_UNEXPLORED
            } else if !vision.visible(tile) {
                FOG_EXPLORED
            } else {
                continue;
            };
            // Exact shared edges: translucent rects that overlap draw
            // double-dark seams, so each tile ends where the next begins.
            let a = game.camera.to_screen(vec2(x as f32, y as f32)).floor();
            let b = game
                .camera
                .to_screen(vec2((x + 1) as f32, (y + 1) as f32))
                .floor();
            draw_rectangle(a.x, a.y, b.x - a.x, b.y - a.y, cover);
        }
    }
}

fn visible_tiles(game: &Game) -> (TilePos, TilePos) {
    let (lo, hi) = game.camera.world_rect();
    let min = TilePos::new((lo.x.floor() as i32).max(0), (lo.y.floor() as i32).max(0));
    let max = TilePos::new(
        (hi.x.ceil() as i32).min(game.state.map().width()),
        (hi.y.ceil() as i32).min(game.state.map().height()),
    );
    (min, max)
}

fn draw_tiles(game: &Game, sprites: &Sprites) {
    let zoom = game.camera.zoom;
    let size = zoom.ceil() + 1.0; // slight overlap kills seam hairlines
    let (min, max) = visible_tiles(game);
    for y in min.y..max.y {
        for x in min.x..max.x {
            let Some(tile) = game.state.map().tile(TilePos::new(x, y)) else {
                continue;
            };
            let screen = game.camera.to_screen(vec2(x as f32, y as f32));
            // Position hashes drive all variety: deterministic, no state.
            let h = (x.wrapping_mul(31).wrapping_add(y.wrapping_mul(17))) as usize;
            let variant = h % 6;
            draw_texture_ex(
                sprites.texture(),
                screen.x.floor(),
                screen.y.floor(),
                WHITE,
                DrawTextureParams {
                    dest_size: Some(vec2(size, size)),
                    source: Some(sprites.ground(variant)),
                    ..Default::default()
                },
            );
            // Ground dressing: rubble tiles always get wreckage; plain
            // ground gets a sparse scatter (~4%) of the smaller decals.
            let decal = if tile.cosmetic == 1 {
                Some(sprites.decal(3))
            } else if tile.terrain == oxide_sim::map::Terrain::Ground && h.is_multiple_of(23) {
                Some(sprites.decal(h / 23 % 3))
            } else {
                None
            };
            if let Some(source) = decal {
                draw_texture_ex(
                    sprites.texture(),
                    screen.x.floor(),
                    screen.y.floor(),
                    WHITE,
                    DrawTextureParams {
                        dest_size: Some(vec2(size, size)),
                        source: Some(source),
                        ..Default::default()
                    },
                );
            }
            // Rocks cast a soft skirt onto neighboring ground.
            if tile.terrain == oxide_sim::map::Terrain::Ground {
                for (dx, dy, rotation) in [
                    (0, -1, 0.0f32),
                    (1, 0, std::f32::consts::FRAC_PI_2),
                    (0, 1, std::f32::consts::PI),
                    (-1, 0, 3.0 * std::f32::consts::FRAC_PI_2),
                ] {
                    let neighbor = TilePos::new(x + dx, y + dy);
                    let rocky = game
                        .state
                        .map()
                        .tile(neighbor)
                        .is_some_and(|t| t.terrain == oxide_sim::map::Terrain::Rock);
                    if rocky {
                        draw_texture_ex(
                            sprites.texture(),
                            screen.x.floor(),
                            screen.y.floor(),
                            WHITE,
                            DrawTextureParams {
                                dest_size: Some(vec2(size, size)),
                                source: Some(sprites.rock_skirt()),
                                rotation,
                                ..Default::default()
                            },
                        );
                    }
                }
            }
            // Scrap draws at its live amount only in sight; unseen ground
            // shows what the player remembers (frozen, like ghosts).
            let pos = TilePos::new(x, y);
            let scrap = if game.overlay || game.my_vision().visible(pos) {
                tile.scrap
            } else {
                game.my_vision().remembered_scrap(pos)
            };
            // Wrecks follow the same sight rule; a live node or rock
            // outranks the junk visually.
            let wreck = if game.overlay || game.my_vision().visible(pos) {
                tile.wreck
            } else {
                game.my_vision().remembered_wreck(pos)
            };
            let (overlay, flip) = match (tile.terrain, scrap) {
                (oxide_sim::map::Terrain::Rock, _) => (Some(sprites.rock(h % 4)), h % 7 < 3),
                (_, 0) if wreck > 0 => (Some(sprites.wreck_pile()), h % 5 < 2),
                (_, 0) => (None, false),
                (_, s) => (Some(sprites.scrap(s, SCRAP_NODE_AMOUNT)), false),
            };
            if let Some(source) = overlay {
                draw_texture_ex(
                    sprites.texture(),
                    screen.x.floor(),
                    screen.y.floor(),
                    WHITE,
                    DrawTextureParams {
                        dest_size: Some(vec2(size, size)),
                        source: Some(source),
                        flip_x: flip,
                        ..Default::default()
                    },
                );
            }
        }
    }
}

const GHOST_TINT: Color = color_u8!(150, 150, 165, 210);

/// Battle scars: scorch decals where buildings died, fading over ~20s.
fn draw_scorches(game: &Game, sprites: &Sprites) {
    let zoom = game.camera.zoom;
    for (at, age) in &game.scorches {
        let alpha = (1.0 - age / 20.0).clamp(0.0, 1.0) * 0.85;
        let size = zoom * 2.4;
        let screen = game.camera.to_screen(*at);
        draw_texture_ex(
            sprites.texture(),
            screen.x - size * 0.5,
            screen.y - size * 0.5,
            Color::new(1.0, 1.0, 1.0, alpha),
            DrawTextureParams {
                dest_size: Some(vec2(size, size)),
                source: Some(sprites.scorch()),
                ..Default::default()
            },
        );
    }
}

fn draw_buildings(game: &Game, sprites: &Sprites) {
    let zoom = game.camera.zoom;
    // Live enemy buildings only where we have sight; remembered ghosts
    // cover explored-but-unseen ground (skipped in the omniscient overlay).
    if !game.overlay {
        for ghost in game.my_vision().ghosts() {
            let (w, h) = ghost.kind.stats().size;
            let visible = (0..h)
                .flat_map(|dy| (0..w).map(move |dx| ghost.anchor.offset(dx, dy)))
                .any(|t| game.my_vision().visible(t));
            if visible {
                continue; // the live building (or its absence) is on show
            }
            let faction = game.state.player(ghost.owner).faction;
            let screen = game
                .camera
                .to_screen(vec2(ghost.anchor.x as f32, ghost.anchor.y as f32));
            // A remembered site stays translucent scaffolding until its
            // completion has actually been observed.
            let tint = if ghost.built {
                GHOST_TINT
            } else {
                Color::new(GHOST_TINT.r, GHOST_TINT.g, GHOST_TINT.b, GHOST_TINT.a * 0.5)
            };
            draw_texture_ex(
                sprites.texture(),
                screen.x,
                screen.y,
                tint,
                DrawTextureParams {
                    dest_size: Some(vec2(w as f32 * zoom, h as f32 * zoom)),
                    source: Some(sprites.building(ghost.kind, faction)),
                    ..Default::default()
                },
            );
        }
    }
    for building in game.state.buildings() {
        if building.player != game.human
            && !game.overlay
            && !building.tiles().any(|t| game.my_vision().visible(t))
        {
            continue;
        }
        let faction = game.state.player(building.player).faction;
        let screen = game
            .camera
            .to_screen(vec2(building.anchor.x as f32, building.anchor.y as f32));
        let (w, h) = building.kind.stats().size;
        let dest = vec2(w as f32 * zoom, h as f32 * zoom);
        // Sites render translucent — scaffolding, not structure.
        let tint = if building.built {
            WHITE
        } else {
            Color::new(1.0, 1.0, 1.0, 0.45)
        };
        draw_texture_ex(
            sprites.texture(),
            screen.x,
            screen.y,
            tint,
            DrawTextureParams {
                dest_size: Some(dest),
                source: Some(sprites.building(building.kind, faction)),
                ..Default::default()
            },
        );
        if building.built && building.kind == oxide_sim::BuildingKind::Foundry {
            // The melt pool breathes: a soft faction-tinted pulse.
            let pulse = ((get_time() * 2.6 + f64::from(building.id.0)).sin() * 0.5 + 0.5) as f32;
            let glow = match faction {
                oxide_sim::Faction::Ferrous => Color::new(0.97, 0.62, 0.45, 0.10 + 0.10 * pulse),
                oxide_sim::Faction::Cupric => Color::new(0.55, 0.87, 0.78, 0.10 + 0.10 * pulse),
            };
            draw_circle(
                screen.x + dest.x * 0.5,
                screen.y + dest.y * 0.5,
                dest.x * 0.22 * (1.0 + 0.08 * pulse),
                glow,
            );
        }
        if !building.built {
            // Construction progress in bone, distinct from training amber.
            let ticks = building
                .kind
                .stats()
                .construction
                .map(|c| c.build_ticks)
                .unwrap_or(1);
            let fraction = building.progress as f32 / ticks as f32;
            draw_rectangle(screen.x, screen.y + dest.y + 3.0, dest.x, 4.0, HP_BACK);
            draw_rectangle(
                screen.x,
                screen.y + dest.y + 3.0,
                dest.x * fraction,
                4.0,
                BONE,
            );
        }
        if game.selection.building == Some(building.id) {
            draw_rectangle_lines(
                screen.x - 2.0,
                screen.y - 2.0,
                dest.x + 4.0,
                dest.y + 4.0,
                3.0,
                BONE,
            );
        }
        let max_hp = building.kind.stats().max_hp;
        if building.hp < max_hp {
            hp_bar(screen.x, screen.y - 8.0, dest.x, building.hp, max_hp);
        }
        // Production progress, drawn under the works.
        if let Some(kind) = building.queue.front() {
            let fraction = building.progress as f32 / kind.stats().train_ticks as f32;
            draw_rectangle(screen.x, screen.y + dest.y + 3.0, dest.x, 4.0, HP_BACK);
            draw_rectangle(
                screen.x,
                screen.y + dest.y + 3.0,
                dest.x * fraction,
                4.0,
                SCRAP_COLOR,
            );
        }
    }
}

fn draw_units(game: &Game, sprites: &Sprites, alpha: f32) {
    // Two passes: ground bodies first, then everything airborne above
    // them — each flyer casts an offset shadow so altitude reads even
    // when nothing overlaps.
    draw_unit_pass(game, sprites, alpha, oxide_sim::stats::Domain::Ground);
    draw_unit_pass(game, sprites, alpha, oxide_sim::stats::Domain::Air);
}

fn draw_unit_pass(game: &Game, sprites: &Sprites, alpha: f32, domain: oxide_sim::stats::Domain) {
    let zoom = game.camera.zoom;
    let airborne = domain == oxide_sim::stats::Domain::Air;
    for unit in game.state.units() {
        if unit.kind.stats().domain != domain {
            continue;
        }
        if unit.player != game.human && !game.overlay && !game.my_vision().visible(unit.tile()) {
            continue;
        }
        let faction = game.state.player(unit.player).faction;
        let pos = game.draw_pos(unit.id, unit.pos, alpha);
        let mut screen = game.camera.to_screen(pos);
        let dest = zoom * 1.05;
        if airborne {
            let shadow = zoom * 0.9;
            draw_texture_ex(
                sprites.texture(),
                screen.x - shadow * 0.5 + zoom * 0.16,
                screen.y - shadow * 0.5 + zoom * 0.26,
                WHITE,
                DrawTextureParams {
                    dest_size: Some(vec2(shadow, shadow)),
                    source: Some(sprites.air_shadow()),
                    ..Default::default()
                },
            );
            // The body rides visibly above its shadow.
            screen.y -= zoom * 0.18;
        }
        let selected = game.selection.units.contains(&unit.id);
        if selected {
            draw_circle_lines(
                screen.x,
                screen.y,
                unit.kind.stats().radius.to_num::<f32>() * zoom + 4.0,
                2.0,
                BONE,
            );
        } else if unit.player != game.human && !game.state.hostile(game.human, unit.player) {
            // Teammates wear a soft whitened ring — same language as the
            // minimap's ally lift, because two teams can field the same
            // faction and sprite color alone cannot say friend or foe.
            draw_circle_lines(
                screen.x,
                screen.y,
                unit.kind.stats().radius.to_num::<f32>() * zoom + 3.0,
                1.5,
                Color::new(0.95, 0.95, 0.9, 0.55),
            );
        }
        draw_texture_ex(
            sprites.texture(),
            screen.x - dest * 0.5,
            screen.y - dest * 0.5,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(dest, dest)),
                source: Some(sprites.unit(unit.kind, faction)),
                rotation: game.facing.get(&unit.id.0).copied().unwrap_or(0.0),
                ..Default::default()
            },
        );
        if unit.kind == UnitKind::Harvester && unit.carrying > 0 {
            draw_circle(screen.x, screen.y, zoom * 0.09, SCRAP_COLOR);
        }
        let max_hp = unit.kind.stats().max_hp;
        if unit.hp < max_hp {
            let w = zoom * 0.8;
            hp_bar(
                screen.x - w * 0.5,
                screen.y - zoom * 0.62,
                w,
                unit.hp,
                max_hp,
            );
        }
    }
}

/// A little pennant marking a selected building's rally tile.
fn draw_rally_flag(game: &Game, rally: TilePos, zoom: f32) {
    let base = game
        .camera
        .to_screen(vec2(rally.x as f32 + 0.5, rally.y as f32 + 0.5));
    let pole_top = base - vec2(0.0, zoom * 0.7);
    draw_line(base.x, base.y, pole_top.x, pole_top.y, 2.0, BONE);
    draw_triangle(
        pole_top,
        pole_top + vec2(zoom * 0.45, zoom * 0.15),
        pole_top + vec2(0.0, zoom * 0.3),
        SCRAP_COLOR,
    );
    draw_circle(base.x, base.y, 3.0, BONE);
}

fn hp_bar(x: f32, y: f32, w: f32, hp: u32, max_hp: u32) {
    let fraction = hp as f32 / max_hp as f32;
    draw_rectangle(x, y, w, 3.0, HP_BACK);
    let color = if fraction < 0.34 { DANGER } else { BONE };
    draw_rectangle(x, y, w * fraction, 3.0, color);
}

fn draw_fx(game: &Game, sprites: &Sprites) {
    let sees = |p: Vec2| {
        game.my_vision()
            .visible(TilePos::new(p.x.floor() as i32, p.y.floor() as i32))
    };
    for fx in &game.fx {
        // A beam needs BOTH endpoints in sight: a half-fogged laser would
        // pinpoint an unseen combatant at its far end.
        let in_sight = match fx.kind {
            EffectKind::Laser { from, to, .. } => sees(from) && sees(to),
            EffectKind::Puff { at } => sees(at),
            EffectKind::Burst { at, .. } => sees(at),
            // Own-order acknowledgments always show; fogged targets are
            // already impossible to order onto.
            EffectKind::Ping { .. } => true,
        };
        if !game.overlay && !in_sight {
            continue;
        }
        match fx.kind {
            EffectKind::Laser { heavy, from, to } => {
                let a = game.camera.to_screen(from);
                let b = game.camera.to_screen(to);
                let fade = (1.0 - fx.age / 0.15).clamp(0.0, 1.0);
                let w = if heavy { 2.0 } else { 1.0 };
                // Wide glow under a hot core.
                draw_line(
                    a.x,
                    a.y,
                    b.x,
                    b.y,
                    7.0 * w * fade.max(0.3),
                    Color::new(0.95, 0.75, 0.5, 0.22 * fade),
                );
                draw_line(
                    a.x,
                    a.y,
                    b.x,
                    b.y,
                    2.5 * w * fade.max(0.2),
                    Color::new(0.98, 0.93, 0.8, fade),
                );
                if fx.age < 0.07 {
                    let dir = b - a;
                    let rotation = dir.y.atan2(dir.x) + std::f32::consts::FRAC_PI_2;
                    let flash = game.camera.zoom * 0.5;
                    draw_texture_ex(
                        sprites.texture(),
                        a.x - flash * 0.5,
                        a.y - flash * 0.5,
                        WHITE,
                        DrawTextureParams {
                            dest_size: Some(vec2(flash, flash)),
                            source: Some(sprites.muzzle_flash()),
                            rotation,
                            ..Default::default()
                        },
                    );
                }
            }
            EffectKind::Puff { at } => {
                let center = game.camera.to_screen(at);
                let fade = 1.0 - fx.age / 0.4;
                let radius = game.camera.zoom * (0.15 + fx.age * 1.6);
                let color = Color::new(0.9, 0.88, 0.84, 0.7 * fade.clamp(0.0, 1.0));
                draw_circle_lines(center.x, center.y, radius, 2.0, color);
            }
            EffectKind::Burst { at, radius } => {
                // The bloom grows toward the splash radius and fades —
                // the player reads exactly the area that just got hit.
                let center = game.camera.to_screen(at);
                let progress = (fx.age / 0.35).clamp(0.0, 1.0);
                let size = game.camera.zoom * radius * 2.0 * (0.4 + 0.6 * progress);
                let alpha = 1.0 - progress;
                draw_texture_ex(
                    sprites.texture(),
                    center.x - size * 0.5,
                    center.y - size * 0.5,
                    Color::new(1.0, 1.0, 1.0, alpha),
                    DrawTextureParams {
                        dest_size: Some(vec2(size, size)),
                        source: Some(sprites.burst()),
                        ..Default::default()
                    },
                );
            }
            EffectKind::Ping { .. } => {} // drawn above the fog, in draw_pings
        }
    }
}

/// Radar blips, drawn above the fog: contacts without identity from the
/// Array's outer ring — the player's own intel, like pings.
fn draw_blips(game: &Game) {
    if game.overlay {
        return; // the omniscient overlay already shows the real machines
    }
    let zoom = game.camera.zoom;
    for &tile in game.my_vision().contacts() {
        let center = game
            .camera
            .to_screen(vec2(tile.x as f32 + 0.5, tile.y as f32 + 0.5));
        let r = zoom * 0.3;
        // A hollow diamond: unmistakably "something", deliberately not
        // any faction's shape or color.
        let pts = [
            vec2(center.x, center.y - r),
            vec2(center.x + r, center.y),
            vec2(center.x, center.y + r),
            vec2(center.x - r, center.y),
        ];
        for i in 0..4 {
            let a = pts[i];
            let b = pts[(i + 1) % 4];
            draw_line(a.x, a.y, b.x, b.y, 2.0, BONE_FAINT);
        }
        draw_circle(center.x, center.y, 2.0, BONE_FAINT);
    }
}

/// Order-acknowledgment rings, drawn above the fog: they are the player's
/// own intent echoed back, not world intel to be hidden.
fn draw_pings(game: &Game) {
    for fx in &game.fx {
        let EffectKind::Ping { at, kind } = fx.kind else {
            continue;
        };
        let center = game.camera.to_screen(at);
        let progress = (fx.age / 0.5).clamp(0.0, 1.0);
        let radius = game.camera.zoom * (0.65 * (1.0 - progress) + 0.12);
        let base = match kind {
            crate::game::PingKind::Move => color_u8!(120, 200, 130, 255),
            crate::game::PingKind::Attack => DANGER,
            crate::game::PingKind::Harvest => SCRAP_COLOR,
            crate::game::PingKind::Rally => BONE,
            crate::game::PingKind::Spawn => color_u8!(150, 210, 235, 255),
        };
        let color = Color::new(base.r, base.g, base.b, 1.0 - progress * 0.7);
        draw_circle_lines(center.x, center.y, radius, 2.5, color);
    }
}

/// The selected own building's rally flag, above the fog for the same
/// reason as pings.
fn draw_rally_marker(game: &Game) {
    if let Some(id) = game.selection.building
        && let Some(building) = game.state.building(id)
        && building.player == game.human
        && let Some(rally) = building.rally
    {
        draw_rally_flag(game, rally, game.camera.zoom);
    }
}

fn draw_drag_rect(game: &Game, input: &InputState) {
    if let Some(origin) = input.drag_origin {
        let now = input.mouse;
        if origin.distance(now) > crate::input::drag_threshold() {
            let lo = origin.min(now);
            let size = (origin - now).abs();
            draw_rectangle_lines(lo.x, lo.y, size.x, size.y, 1.5, BONE);
            draw_rectangle(
                lo.x,
                lo.y,
                size.x,
                size.y,
                Color::new(0.9, 0.88, 0.84, 0.08),
            );
            // Live preview: who would this select?
            let a = game.camera.to_world(lo);
            let b = game.camera.to_world(lo + size);
            for unit in game.state.units() {
                if unit.player != game.human {
                    continue;
                }
                let p = vec2(unit.pos.x.to_num::<f32>(), unit.pos.y.to_num::<f32>());
                if p.x >= a.x && p.x <= b.x && p.y >= a.y && p.y <= b.y {
                    let screen = game.camera.to_screen(p);
                    draw_circle_lines(
                        screen.x,
                        screen.y,
                        unit.kind.stats().radius.to_num::<f32>() * game.camera.zoom + 3.0,
                        1.5,
                        BONE_FAINT,
                    );
                }
            }
        }
    }
}

fn draw_overlay(game: &Game, alpha: f32) {
    let (min, max) = visible_tiles(game);
    for x in min.x..=max.x {
        let a = game.camera.to_screen(vec2(x as f32, min.y as f32));
        let b = game.camera.to_screen(vec2(x as f32, max.y as f32));
        draw_line(a.x, a.y, b.x, b.y, 1.0, BONE_FAINT);
    }
    for y in min.y..=max.y {
        let a = game.camera.to_screen(vec2(min.x as f32, y as f32));
        let b = game.camera.to_screen(vec2(max.x as f32, y as f32));
        draw_line(a.x, a.y, b.x, b.y, 1.0, BONE_FAINT);
    }
    for unit in game.state.units() {
        let pos = game.draw_pos(unit.id, unit.pos, alpha);
        let screen = game.camera.to_screen(pos);
        draw_text(
            format!("u{} {}hp", unit.id.0, unit.hp),
            screen.x + 8.0,
            screen.y - 8.0,
            16.0,
            BONE,
        );
        if let Some(path) = &unit.path {
            let mut previous = screen;
            for waypoint in path.waypoints.iter().skip(path.next as usize) {
                let next = game
                    .camera
                    .to_screen(vec2(waypoint.x as f32 + 0.5, waypoint.y as f32 + 0.5));
                draw_line(previous.x, previous.y, next.x, next.y, 1.0, BONE_FAINT);
                previous = next;
            }
        }
    }
    let info = format!(
        "tick {}  fps {}  zoom {:.0}  center ({:.1},{:.1})",
        game.state.current_tick(),
        get_fps(),
        game.camera.zoom,
        game.camera.center.x,
        game.camera.center.y,
    );
    let s = ui_scale();
    draw_text(&info, screen_width() - 420.0 * s, 54.0 * s, 18.0 * s, BONE);
}

fn draw_hud(game: &Game, input: &InputState) {
    let s = ui_scale();
    // Top bar.
    draw_rectangle(0.0, 0.0, screen_width(), 32.0 * s, PANEL);
    let me = game.state.player(game.human);
    let my_units = game
        .state
        .units()
        .iter()
        .filter(|u| u.player == game.human)
        .count();
    draw_text(
        format!("SCRAP {}", me.scrap),
        12.0 * s,
        22.0 * s,
        22.0 * s,
        SCRAP_COLOR,
    );
    draw_text(
        format!("UNITS {my_units}"),
        150.0 * s,
        22.0 * s,
        22.0 * s,
        BONE,
    );
    draw_text(
        format!("TICK {}", game.state.current_tick()),
        270.0 * s,
        22.0 * s,
        22.0 * s,
        BONE_FAINT,
    );
    if game.paused {
        draw_text("PAUSED (P)", 420.0 * s, 22.0 * s, 22.0 * s, DANGER);
    } else if (game.speed - 1.0).abs() > f64::EPSILON {
        draw_text(
            format!("SPEED x{:.2}", game.speed),
            420.0 * s,
            22.0 * s,
            22.0 * s,
            SCRAP_COLOR,
        );
    }

    let mut panel_shown = false;
    // Selection panel.
    if let Some(id) = game.selection.building {
        if let Some(building) = game.state.building(id) {
            let queue: Vec<&str> = building.queue.iter().map(|k| k.name()).collect();
            let stats = building.kind.stats();
            let name = building.kind.name().to_uppercase();
            let mut line = format!("{name} {}/{} hp", building.hp, stats.max_hp);
            if !building.built {
                line.push_str("   under construction   X: scrap site");
                panel_rows_packed(std::slice::from_ref(&line), 0);
            } else if !stats.produces.is_empty() {
                // Number keys train; the list is the seat's own roster
                // (the other faction's variants never show).
                let faction = game.state.player(game.human).faction;
                let slots: Vec<String> = stats
                    .produces
                    .iter()
                    .filter(|k| k.faction().is_none_or(|f| f == faction))
                    .enumerate()
                    .map(|(i, k)| format!("{}: {} ({})", i + 1, k.name(), k.stats().cost))
                    .collect();
                let used = panel_rows_packed(&slots, 0);
                let header = vec![line, format!("queue [{}]", queue.join(", "))];
                panel_rows_packed(&header, used);
            } else {
                panel_line(&line);
            }
            panel_shown = true;
        }
    } else if !game.selection.units.is_empty() {
        let has_builder = game.selection.units.iter().any(|id| {
            game.state
                .unit(*id)
                .is_some_and(|u| u.kind == UnitKind::Harvester)
        });
        let mut line_items = vec![
            format!("{} unit(s) selected", game.selection.units.len()),
            "X: stop".to_string(),
            "R: patrol".to_string(),
        ];
        if has_builder {
            line_items.push("B: build".to_string());
        }
        if input.build_menu {
            let palette: Vec<String> = crate::input::BUILD_PALETTE
                .iter()
                .enumerate()
                .map(|(i, k)| {
                    let cost = k.stats().construction.map(|c| c.cost).unwrap_or(0);
                    format!("{}: {} ({})", i + 1, k.name(), cost)
                })
                .collect();
            let used = panel_rows_packed(&palette, 0);
            panel_rows_packed(&line_items, used);
        } else {
            panel_rows_packed(&line_items, 0);
        }
        panel_shown = true;
    }

    // Controls hint — it lives in the same bottom band as the selection
    // panel, so it yields whenever a panel is up (the panel carries its
    // own key prompts).
    if !panel_shown {
        let hint =
            "LMB select · RMB move/engage · 1-9 train · B build · arrows pan · Esc menu · F1 debug";
        let width = measure_text(hint, None, (16.0 * s) as u16, 1.0).width;
        draw_text(
            hint,
            screen_width() - width - 10.0 * s,
            screen_height() - 10.0 * s,
            16.0 * s,
            BONE_FAINT,
        );
    }

    // Toasts: rejected orders and stalled units, newest at the bottom.
    for (i, toast) in game.toasts.iter().rev().take(3).enumerate() {
        let fade = (1.0 - (toast.age - 1.5).max(0.0)).clamp(0.0, 1.0);
        let y = screen_height() - (60.0 + 24.0 * i as f32) * s;
        let color = Color::new(0.92, 0.5, 0.45, fade);
        draw_text(&toast.text, 12.0 * s, y, 20.0 * s, color);
    }

    // Starter hints, until the player has done each thing once.
    let mut hint_y = 52.0 * s;
    if !game.hinted_train {
        draw_text(
            "H trains a Harvester at your Foundry - keep scrap flowing",
            12.0 * s,
            hint_y,
            18.0 * s,
            BONE_FAINT,
        );
        hint_y += 22.0 * s;
    }
    if !game.hinted_fight {
        draw_text(
            "Right-click sends your machines - they fight whatever they meet",
            12.0 * s,
            hint_y,
            18.0 * s,
            BONE_FAINT,
        );
    }

    // Endgame banner.
    if let Some(result) = game.state.result() {
        let text = match result {
            GameResult::Victory { .. } => {
                // Name the winners: one seat by name, a team by roster.
                let names: Vec<String> = game
                    .state
                    .winners()
                    .into_iter()
                    .map(|p| game.state.player(p).name.to_uppercase())
                    .collect();
                format!("{} WINS", names.join(" & "))
            }
            GameResult::Draw => "MUTUAL DESTRUCTION".to_string(),
        };
        let size = 56.0 * s;
        let dims = measure_text(&text, None, size as u16, 1.0);
        let x = (screen_width() - dims.width) * 0.5;
        let y = screen_height() * 0.4;
        draw_rectangle(
            x - 24.0 * s,
            y - 48.0 * s,
            dims.width + 48.0 * s,
            100.0 * s,
            PANEL,
        );
        draw_text(&text, x, y, size, SCRAP_COLOR);
        let hint = "Esc — menu";
        let hint_dims = measure_text(hint, None, (20.0 * s) as u16, 1.0);
        draw_text(
            hint,
            (screen_width() - hint_dims.width) * 0.5,
            y + 34.0 * s,
            20.0 * s,
            BONE_FAINT,
        );
    }
}

fn panel_line(text: &str) {
    panel_row(text, 0);
}

/// A bottom panel band; `row` 0 is the lowest, higher rows stack above.
fn panel_row(text: &str, row: usize) {
    let s = ui_scale();
    let base = screen_height() - 36.0 * s * (row as f32 + 1.0);
    draw_rectangle(0.0, base, screen_width(), 36.0 * s, PANEL);
    draw_text(text, 12.0 * s, base + 24.0 * s, 20.0 * s, BONE);
}

/// Lays `items` into as many panel rows as they need, packed greedily
/// to fit left of the minimap, stacked above row `first`. Returns how
/// many rows it used. A single long line would run off the right edge
/// (and under the minimap) at retina scale — palette and production
/// slots overflow real screens without this.
fn panel_rows_packed(items: &[String], first: usize) -> usize {
    let s = ui_scale();
    let sep = "   ";
    let limit = (screen_width() - 240.0 * s).max(320.0 * s);
    let fits = |line: &str| measure_text(line, None, (20.0 * s) as u16, 1.0).width < limit;
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for item in items {
        let candidate = if current.is_empty() {
            item.clone()
        } else {
            format!("{current}{sep}{item}")
        };
        if fits(&candidate) || current.is_empty() {
            current = candidate;
        } else {
            lines.push(std::mem::take(&mut current));
            current = item.clone();
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    // Stack bottom-up: the first packed line sits highest so reading
    // order stays top-to-bottom.
    let n = lines.len();
    for (i, line) in lines.iter().enumerate() {
        panel_row(line, first + (n - 1 - i));
    }
    n
}

// --- Minimap ------------------------------------------------------------

const MINIMAP_MAX: Vec2 = vec2(220.0, 150.0);
const MINI_VOID: Color = color_u8!(10, 10, 13, 255);
const MINI_GROUND: Color = color_u8!(44, 44, 52, 255);
const MINI_ROCK: Color = color_u8!(84, 84, 96, 255);

/// Minimap allegiance color: faction color, lifted toward white for
/// teammates — "friendly, not yours" at a glance (a 2v2 fields the same
/// faction on both sides, so tint alone can't say friend or foe).
fn mini_entity_color(game: &Game, owner: oxide_sim::PlayerId) -> Color {
    let base = mini_faction_color(game.state.player(owner).faction);
    if owner != game.human && !game.state.hostile(game.human, owner) {
        Color::new(
            base.r * 0.45 + 0.55,
            base.g * 0.45 + 0.55,
            base.b * 0.45 + 0.55,
            base.a,
        )
    } else {
        base
    }
}

fn dim(color: Color) -> Color {
    Color::new(color.r * 0.55, color.g * 0.55, color.b * 0.55, color.a)
}

fn mini_faction_color(faction: oxide_sim::Faction) -> Color {
    match faction {
        oxide_sim::Faction::Ferrous => color_u8!(196, 87, 59, 255),
        oxide_sim::Faction::Cupric => color_u8!(63, 148, 130, 255),
    }
}

/// Where the minimap sits (bottom-right, above the hint line) for a map of
/// `map_w`×`map_h` tiles in a `viewport`-pixel window. Pure — shared with
/// input hit-testing and unit tests.
pub fn minimap_rect_for(map_w: i32, map_h: i32, viewport: Vec2) -> Rect {
    minimap_rect_scaled(map_w, map_h, viewport, ui_scale())
}

/// Testable core of [`minimap_rect_for`] (no window queries).
pub fn minimap_rect_scaled(map_w: i32, map_h: i32, viewport: Vec2, s: f32) -> Rect {
    let mw = map_w as f32;
    let mh = map_h as f32;
    let scale = (MINIMAP_MAX.x * s / mw).min(MINIMAP_MAX.y * s / mh);
    let (w, h) = (mw * scale, mh * scale);
    Rect::new(viewport.x - w - 12.0 * s, viewport.y - h - 34.0 * s, w, h)
}

/// Where the minimap sits this frame.
pub fn minimap_rect(game: &Game) -> Rect {
    minimap_rect_for(
        game.state.map().width(),
        game.state.map().height(),
        game.camera.viewport(),
    )
}

/// The world point under a screen position, if it lies on the minimap —
/// how clicks jump the camera (and where armed attack-moves land).
pub fn minimap_world_at(game: &Game, screen: Vec2) -> Option<Vec2> {
    minimap_world_in(minimap_rect(game), game.state.map().width(), screen)
}

/// Testable core of [`minimap_world_at`] (no window queries).
pub fn minimap_world_in(rect: Rect, map_w: i32, screen: Vec2) -> Option<Vec2> {
    if !rect.contains(screen) {
        return None;
    }
    let scale = rect.w / map_w as f32;
    Some(vec2(
        (screen.x - rect.x) / scale,
        (screen.y - rect.y) / scale,
    ))
}

/// The whole war at a glance, under the same fog rules as the world view
/// (and, like everything else, omniscient while the F1 overlay is up).
fn draw_minimap(game: &Game) {
    let rect = minimap_rect(game);
    let scale = rect.w / game.state.map().width() as f32;
    let omniscient = game.overlay;
    let vision = game.my_vision();
    draw_rectangle(
        rect.x - 3.0,
        rect.y - 3.0,
        rect.w + 6.0,
        rect.h + 6.0,
        PANEL,
    );

    let cell = scale.ceil();
    for (pos, tile) in game.state.map().iter() {
        let (explored, visible) = if omniscient {
            (true, true)
        } else {
            (vision.explored(pos), vision.visible(pos))
        };
        let color = if !explored {
            MINI_VOID
        } else {
            // Same memory rule as the world view: live scrap in sight,
            // last-seen scrap under the dim.
            let scrap = if visible {
                tile.scrap
            } else {
                vision.remembered_scrap(pos)
            };
            let base = match (tile.terrain, scrap) {
                (oxide_sim::map::Terrain::Rock, _) => MINI_ROCK,
                (_, 0) => MINI_GROUND,
                (_, _) => SCRAP_COLOR,
            };
            if visible { base } else { dim(base) }
        };
        draw_rectangle(
            rect.x + pos.x as f32 * scale,
            rect.y + pos.y as f32 * scale,
            cell,
            cell,
            color,
        );
    }

    if !omniscient {
        for ghost in vision.ghosts() {
            let (w, h) = ghost.kind.stats().size;
            let color = dim(mini_faction_color(game.state.player(ghost.owner).faction));
            draw_rectangle(
                rect.x + ghost.anchor.x as f32 * scale,
                rect.y + ghost.anchor.y as f32 * scale,
                w as f32 * scale,
                h as f32 * scale,
                color,
            );
        }
    }
    for building in game.state.buildings() {
        let seen = omniscient
            || building.player == game.human
            || building.tiles().any(|t| vision.visible(t));
        if !seen {
            continue;
        }
        let (w, h) = building.kind.stats().size;
        draw_rectangle(
            rect.x + building.anchor.x as f32 * scale,
            rect.y + building.anchor.y as f32 * scale,
            w as f32 * scale,
            h as f32 * scale,
            mini_entity_color(game, building.player),
        );
    }
    for unit in game.state.units() {
        let seen = omniscient || unit.player == game.human || vision.visible(unit.tile());
        if !seen {
            continue;
        }
        let dot = (scale * 0.7).max(2.0);
        draw_rectangle(
            rect.x + unit.pos.x.to_num::<f32>() * scale - dot * 0.5,
            rect.y + unit.pos.y.to_num::<f32>() * scale - dot * 0.5,
            dot,
            dot,
            mini_entity_color(game, unit.player),
        );
    }

    // Camera frame.
    let (lo, hi) = game.camera.world_rect();
    let x = rect.x + lo.x.max(0.0) * scale;
    let y = rect.y + lo.y.max(0.0) * scale;
    let x2 = rect.x + hi.x.min(game.state.map().width() as f32) * scale;
    let y2 = rect.y + hi.y.min(game.state.map().height() as f32) * scale;
    draw_rectangle_lines(x, y, (x2 - x).max(4.0), (y2 - y).max(4.0), 1.5, BONE);
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEWPORT: Vec2 = vec2(1280.0, 800.0);

    #[test]
    fn minimap_keeps_map_aspect_and_fits_its_budget() {
        for (w, h) in [(40, 24), (26, 16), (44, 20), (48, 30), (256, 256)] {
            let rect = minimap_rect_scaled(w, h, VIEWPORT, 1.0);
            assert!(rect.w <= MINIMAP_MAX.x + 0.01 && rect.h <= MINIMAP_MAX.y + 0.01);
            let map_aspect = w as f32 / h as f32;
            assert!(
                (rect.w / rect.h - map_aspect).abs() < 0.01,
                "{w}x{h} squished to {}x{}",
                rect.w,
                rect.h
            );
        }
    }

    #[test]
    fn minimap_hugs_the_bottom_right_at_any_ui_scale() {
        for s in [1.0, 2.0] {
            let rect = minimap_rect_scaled(40, 24, VIEWPORT, s);
            assert_eq!(rect.x + rect.w, VIEWPORT.x - 12.0 * s);
            assert_eq!(rect.y + rect.h, VIEWPORT.y - 34.0 * s);
        }
    }

    #[test]
    fn minimap_clicks_map_back_to_world_tiles() {
        let (map_w, map_h) = (40, 24);
        let rect = minimap_rect_scaled(map_w, map_h, VIEWPORT, 1.0);
        let scale = rect.w / map_w as f32;
        // The pixel at a tile center's minimap position maps back to it.
        for tile in [(0, 0), (20, 12), (39, 23)] {
            let screen = vec2(
                rect.x + (tile.0 as f32 + 0.5) * scale,
                rect.y + (tile.1 as f32 + 0.5) * scale,
            );
            let world = minimap_world_in(rect, map_w, screen).unwrap();
            assert_eq!((world.x.floor() as i32, world.y.floor() as i32), tile);
        }
        assert!(map_h as f32 * scale <= rect.h + 0.01);
    }

    #[test]
    fn clicks_off_the_minimap_are_not_world_clicks() {
        let rect = minimap_rect_scaled(40, 24, VIEWPORT, 1.0);
        assert!(minimap_world_in(rect, 40, vec2(rect.x - 1.0, rect.y)).is_none());
        assert!(minimap_world_in(rect, 40, vec2(0.0, 0.0)).is_none());
    }
}
