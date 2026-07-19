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

/// Draws one frame.
pub fn draw(game: &Game, sprites: &Sprites, input: &InputState) {
    clear_background(OUTSIDE);
    let alpha = game.render_alpha();
    draw_tiles(game, sprites);
    draw_buildings(game, sprites);
    draw_units(game, sprites, alpha);
    draw_fx(game);
    // The debug overlay is deliberately omniscient; fog only draws without it.
    if game.overlay {
        draw_overlay(game, alpha);
    } else {
        draw_fog(game);
    }
    draw_drag_rect(input);
    draw_hud(game);
    draw_minimap(game);
}

const FOG_UNEXPLORED: Color = color_u8!(13, 13, 17, 255);
const FOG_EXPLORED: Color = color_u8!(13, 13, 17, 130);

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
        (hi.x.ceil() as i32).min(game.state.map.width()),
        (hi.y.ceil() as i32).min(game.state.map.height()),
    );
    (min, max)
}

fn draw_tiles(game: &Game, sprites: &Sprites) {
    let zoom = game.camera.zoom;
    let size = zoom.ceil() + 1.0; // slight overlap kills seam hairlines
    let (min, max) = visible_tiles(game);
    for y in min.y..max.y {
        for x in min.x..max.x {
            let Some(tile) = game.state.map.tile(TilePos::new(x, y)) else {
                continue;
            };
            let screen = game.camera.to_screen(vec2(x as f32, y as f32));
            let params = DrawTextureParams {
                dest_size: Some(vec2(size, size)),
                ..Default::default()
            };
            let variant = ((x * 7 + y * 13) % 3) as usize;
            draw_texture_ex(
                &sprites.ground[variant],
                screen.x.floor(),
                screen.y.floor(),
                WHITE,
                params.clone(),
            );
            // Scrap draws at its live amount only in sight; unseen ground
            // shows what the player remembers (frozen, like ghosts).
            let pos = TilePos::new(x, y);
            let scrap = if game.overlay || game.my_vision().visible(pos) {
                tile.scrap
            } else {
                game.my_vision().remembered_scrap(pos)
            };
            let overlay = match (tile.terrain, scrap) {
                (oxide_sim::map::Terrain::Rock, _) => Some(&sprites.rock),
                (_, 0) => None,
                (_, s) if s * 3 > SCRAP_NODE_AMOUNT * 2 => Some(&sprites.scrap_full),
                (_, s) if s * 3 > SCRAP_NODE_AMOUNT => Some(&sprites.scrap_mid),
                _ => Some(&sprites.scrap_low),
            };
            if let Some(texture) = overlay {
                draw_texture_ex(texture, screen.x.floor(), screen.y.floor(), WHITE, params);
            }
        }
    }
}

const GHOST_TINT: Color = color_u8!(150, 150, 165, 210);

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
            draw_texture_ex(
                sprites.foundry(faction),
                screen.x,
                screen.y,
                GHOST_TINT,
                DrawTextureParams {
                    dest_size: Some(vec2(w as f32 * zoom, h as f32 * zoom)),
                    ..Default::default()
                },
            );
        }
    }
    for building in &game.state.buildings {
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
        draw_texture_ex(
            sprites.foundry(faction),
            screen.x,
            screen.y,
            WHITE,
            DrawTextureParams {
                dest_size: Some(dest),
                ..Default::default()
            },
        );
        if game.selection.building == Some(building.id) {
            draw_rectangle_lines(
                screen.x - 2.0,
                screen.y - 2.0,
                dest.x + 4.0,
                dest.y + 4.0,
                3.0,
                BONE,
            );
            if let Some(rally) = building.rally {
                draw_rally_flag(game, rally, zoom);
            }
        }
        let max_hp = building.kind.stats().max_hp;
        if building.hp < max_hp {
            hp_bar(screen.x, screen.y - 8.0, dest.x, building.hp, max_hp);
        }
        // Production progress, drawn under the works.
        if let Some(kind) = building.queue.first() {
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
    let zoom = game.camera.zoom;
    for unit in &game.state.units {
        if unit.player != game.human && !game.overlay && !game.my_vision().visible(unit.tile()) {
            continue;
        }
        let faction = game.state.player(unit.player).faction;
        let pos = game.draw_pos(unit.id, unit.pos, alpha);
        let screen = game.camera.to_screen(pos);
        let dest = zoom * 1.05;
        let selected = game.selection.units.contains(&unit.id);
        if selected {
            draw_circle_lines(
                screen.x,
                screen.y,
                unit.kind.stats().radius.to_num::<f32>() * zoom + 4.0,
                2.0,
                BONE,
            );
        }
        draw_texture_ex(
            sprites.unit(unit.kind, faction),
            screen.x - dest * 0.5,
            screen.y - dest * 0.5,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(dest, dest)),
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

fn draw_fx(game: &Game) {
    let sees = |p: Vec2| {
        game.my_vision()
            .visible(TilePos::new(p.x.floor() as i32, p.y.floor() as i32))
    };
    for fx in &game.fx {
        // A beam needs BOTH endpoints in sight: a half-fogged laser would
        // pinpoint an unseen combatant at its far end.
        let in_sight = match fx.kind {
            EffectKind::Laser { from, to } => sees(from) && sees(to),
            EffectKind::Puff { at } => sees(at),
        };
        if !game.overlay && !in_sight {
            continue;
        }
        match fx.kind {
            EffectKind::Laser { from, to } => {
                let a = game.camera.to_screen(from);
                let b = game.camera.to_screen(to);
                let fade = 1.0 - fx.age / 0.15;
                let color = Color::new(0.95, 0.9, 0.75, fade.clamp(0.0, 1.0));
                draw_line(a.x, a.y, b.x, b.y, 2.5 * fade.max(0.2), color);
            }
            EffectKind::Puff { at } => {
                let center = game.camera.to_screen(at);
                let fade = 1.0 - fx.age / 0.4;
                let radius = game.camera.zoom * (0.15 + fx.age * 1.6);
                let color = Color::new(0.9, 0.88, 0.84, 0.7 * fade.clamp(0.0, 1.0));
                draw_circle_lines(center.x, center.y, radius, 2.0, color);
            }
        }
    }
}

fn draw_drag_rect(input: &InputState) {
    if let Some(origin) = input.drag_origin {
        let now = input.mouse;
        if origin.distance(now) > 6.0 {
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
    for unit in &game.state.units {
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
        game.state.tick,
        get_fps(),
        game.camera.zoom,
        game.camera.center.x,
        game.camera.center.y,
    );
    draw_text(&info, screen_width() - 420.0, 54.0, 18.0, BONE);
}

fn draw_hud(game: &Game) {
    // Top bar.
    draw_rectangle(0.0, 0.0, screen_width(), 32.0, PANEL);
    let me = game.state.player(game.human);
    let my_units = game
        .state
        .units
        .iter()
        .filter(|u| u.player == game.human)
        .count();
    draw_text(format!("SCRAP {}", me.scrap), 12.0, 22.0, 22.0, SCRAP_COLOR);
    draw_text(format!("UNITS {my_units}"), 150.0, 22.0, 22.0, BONE);
    draw_text(
        format!("TICK {}", game.state.tick),
        270.0,
        22.0,
        22.0,
        BONE_FAINT,
    );
    if game.paused {
        draw_text("PAUSED (P)", 420.0, 22.0, 22.0, DANGER);
    } else if (game.speed - 1.0).abs() > f64::EPSILON {
        draw_text(
            format!("SPEED x{:.2}", game.speed),
            420.0,
            22.0,
            22.0,
            SCRAP_COLOR,
        );
    }

    // Selection panel.
    if let Some(id) = game.selection.building {
        if let Some(building) = game.state.building(id) {
            let queue: Vec<&str> = building
                .queue
                .iter()
                .map(|k| match k {
                    UnitKind::Harvester => "harvester",
                    UnitKind::Sentinel => "sentinel",
                })
                .collect();
            let line = format!(
                "FOUNDRY {}/{} hp   queue [{}]   H: harvester (50)   S: sentinel (75)",
                building.hp,
                building.kind.stats().max_hp,
                queue.join(", "),
            );
            panel_line(&line);
        }
    } else if !game.selection.units.is_empty() {
        panel_line(&format!("{} unit(s) selected", game.selection.units.len()));
    }

    // Controls hint.
    let hint = "LMB select · RMB move/engage · H/S train · arrows pan · Esc menu · F1 debug";
    let width = measure_text(hint, None, 16, 1.0).width;
    draw_text(
        hint,
        screen_width() - width - 10.0,
        screen_height() - 10.0,
        16.0,
        BONE_FAINT,
    );

    // Endgame banner.
    if let Some(result) = game.state.result {
        let text = match result {
            GameResult::Victory { winner } => {
                format!("{} WINS", game.state.player(winner).name.to_uppercase())
            }
            GameResult::Draw => "MUTUAL DESTRUCTION".to_string(),
        };
        let size = 56.0;
        let dims = measure_text(&text, None, size as u16, 1.0);
        let x = (screen_width() - dims.width) * 0.5;
        let y = screen_height() * 0.4;
        draw_rectangle(x - 24.0, y - 48.0, dims.width + 48.0, 100.0, PANEL);
        draw_text(&text, x, y, size, SCRAP_COLOR);
        let hint = "Esc — menu";
        let hint_dims = measure_text(hint, None, 20, 1.0);
        draw_text(
            hint,
            (screen_width() - hint_dims.width) * 0.5,
            y + 34.0,
            20.0,
            BONE_FAINT,
        );
    }
}

fn panel_line(text: &str) {
    draw_rectangle(0.0, screen_height() - 36.0, screen_width(), 36.0, PANEL);
    draw_text(text, 12.0, screen_height() - 12.0, 20.0, BONE);
}

// --- Minimap ------------------------------------------------------------

const MINIMAP_MAX: Vec2 = vec2(220.0, 150.0);
const MINI_VOID: Color = color_u8!(10, 10, 13, 255);
const MINI_GROUND: Color = color_u8!(44, 44, 52, 255);
const MINI_ROCK: Color = color_u8!(84, 84, 96, 255);

fn dim(color: Color) -> Color {
    Color::new(color.r * 0.55, color.g * 0.55, color.b * 0.55, color.a)
}

fn mini_faction_color(faction: oxide_sim::Faction) -> Color {
    match faction {
        oxide_sim::Faction::Ferrous => color_u8!(196, 87, 59, 255),
        oxide_sim::Faction::Cupric => color_u8!(63, 148, 130, 255),
    }
}

/// Where the minimap sits this frame (bottom-right, above the hint line).
pub fn minimap_rect(game: &Game) -> Rect {
    let mw = game.state.map.width() as f32;
    let mh = game.state.map.height() as f32;
    let scale = (MINIMAP_MAX.x / mw).min(MINIMAP_MAX.y / mh);
    let (w, h) = (mw * scale, mh * scale);
    Rect::new(screen_width() - w - 12.0, screen_height() - h - 34.0, w, h)
}

/// The world point under a screen position, if it lies on the minimap —
/// how clicks jump the camera (and where armed attack-moves land).
pub fn minimap_world_at(game: &Game, screen: Vec2) -> Option<Vec2> {
    let rect = minimap_rect(game);
    if !rect.contains(screen) {
        return None;
    }
    let scale = rect.w / game.state.map.width() as f32;
    Some(vec2(
        (screen.x - rect.x) / scale,
        (screen.y - rect.y) / scale,
    ))
}

/// The whole war at a glance, under the same fog rules as the world view
/// (and, like everything else, omniscient while the F1 overlay is up).
fn draw_minimap(game: &Game) {
    let rect = minimap_rect(game);
    let scale = rect.w / game.state.map.width() as f32;
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
    for (pos, tile) in game.state.map.iter() {
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
    for building in &game.state.buildings {
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
            mini_faction_color(game.state.player(building.player).faction),
        );
    }
    for unit in &game.state.units {
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
            mini_faction_color(game.state.player(unit.player).faction),
        );
    }

    // Camera frame.
    let (lo, hi) = game.camera.world_rect();
    let x = rect.x + lo.x.max(0.0) * scale;
    let y = rect.y + lo.y.max(0.0) * scale;
    let x2 = rect.x + hi.x.min(game.state.map.width() as f32) * scale;
    let y2 = rect.y + hi.y.min(game.state.map.height() as f32) * scale;
    draw_rectangle_lines(x, y, (x2 - x).max(4.0), (y2 - y).max(4.0), 1.5, BONE);
}
