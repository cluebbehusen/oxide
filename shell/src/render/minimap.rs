//! The minimap: its corner geometry (one source for drawing, clicks,
//! and the debug protocol's chrome report) and its drawing, which shows
//! live state on visible ground and memories elsewhere, like the world
//! renderer.

use super::*;
use crate::numeric::Fit;
use crate::render::prim::stroke_circle;

const MINIMAP_MAX: Vec2 = vec2(220.0, 150.0);
const MINI_VOID: Color = color_u8!(10, 10, 13, 255);
const MINI_GROUND: Color = color_u8!(44, 44, 52, 255);
const MINI_ROCK: Color = color_u8!(84, 84, 96, 255);
const MINI_PEAK: Color = color_u8!(48, 47, 57, 255);
const MINI_PIT: Color = color_u8!(6, 6, 9, 255);

/// Minimap identity color: the same seat accents the world renderer uses
/// (own faction, cool allies, warm hostiles).
fn mini_entity_color(game: &crate::game::Scene<'_>, owner: oxide_sim::PlayerId) -> Color {
    super::seat_identity_color(game, owner)
}

fn dim(color: Color) -> Color {
    Color::new(color.r * 0.55, color.g * 0.55, color.b * 0.55, color.a)
}

/// The minimap's staleness fade: memories slide toward the dim ground
/// color as they age, matching the world view's alpha fade.
fn stale_toward(color: Color, floor: Color, age: f32) -> Color {
    let t = super::staleness_fade(age);
    Color::new(
        color.r + (floor.r - color.r) * t,
        color.g + (floor.g - color.g) * t,
        color.b + (floor.b - color.b) * t,
        color.a,
    )
}

/// How long since the player last saw this key. Unstamped memories (from
/// loaded saves) start aging now.
fn memory_age(game: &crate::game::Scene<'_>, key: (i32, i32)) -> f32 {
    let mut seen = game.presentation.last_seen.borrow_mut();
    let stamp = *seen
        .entry(key)
        .or_insert_with(|| game.presentation.fx_time());
    game.presentation.fx_time() - stamp
}

/// Where the minimap sits (flush bottom-right, matching the command
/// band's bottom edge) for a map of `map_w`×`map_h` tiles in a
/// `viewport`-pixel window. Shared with input hit-testing and unit tests.
pub fn minimap_rect_for(map_w: i32, map_h: i32, viewport: Vec2) -> Rect {
    minimap_rect_scaled(map_w, map_h, viewport, ui_scale())
}

/// [`minimap_rect_for`] with an explicit UI scale.
pub fn minimap_rect_scaled(map_w: i32, map_h: i32, viewport: Vec2, s: f32) -> Rect {
    let mw = map_w as f32;
    let mh = map_h as f32;
    let scale = (MINIMAP_MAX.x * s / mw).min(MINIMAP_MAX.y * s / mh);
    let (w, h) = (mw * scale, mh * scale);
    Rect::new(viewport.x - w - 12.0 * s, viewport.y - h - 12.0 * s, w, h)
}

/// Where the minimap sits this frame.
pub fn minimap_rect(game: &crate::game::Scene<'_>) -> Rect {
    minimap_rect_for(
        game.state.map().width(),
        game.state.map().height(),
        game.presentation.camera.viewport(),
    )
}

/// The world point under a screen position, if it lies on the minimap.
/// Clicks use it to jump the camera and to aim armed ground orders.
pub fn minimap_world_at(game: &crate::game::Scene<'_>, screen: Vec2) -> Option<Vec2> {
    // The published rect, not a recomputation: hit-testing reads the
    // LayoutModel like all chrome and never touches the window, which
    // keeps the click path headless-testable.
    minimap_world_in(
        game.presentation.layout.get().minimap,
        game.state.map().width(),
        screen,
    )
}

/// The world point a pointer dragging from the minimap steers to:
/// clamped into the published rect, so sliding off an edge holds the
/// view at that edge instead of stalling the pan. `None` while the
/// minimap is hidden.
pub fn minimap_world_clamped(game: &crate::game::Scene<'_>, screen: Vec2) -> Option<Vec2> {
    let rect = game.presentation.layout.get().minimap;
    if rect.w <= 0.0 || rect.h <= 0.0 {
        return None;
    }
    let clamped = vec2(
        screen.x.clamp(rect.x, rect.x + rect.w - 1.0),
        screen.y.clamp(rect.y, rect.y + rect.h - 1.0),
    );
    minimap_world_in(rect, game.state.map().width(), clamped)
}

/// Testable core of [`minimap_world_at`], given the minimap rect.
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

/// The minimap's terrain-and-fog layer: one pixel per tile, uploaded to
/// a texture and drawn as a single scaled quad instead of one quad per
/// tile. Created lazily inside the draw so headless sessions never touch
/// the GPU.
pub(crate) struct MinimapLayer {
    image: Image,
    texture: Texture2D,
}

impl MinimapLayer {
    fn ensure(slot: &mut Option<Self>, w: i32, h: i32) -> &mut Self {
        let (w, h) = (w.max(1).fit::<u16>(), h.max(1).fit::<u16>());
        if slot
            .as_ref()
            .is_none_or(|layer| layer.image.width != w || layer.image.height != h)
        {
            let image = Image::gen_image_color(w, h, MINI_VOID);
            let texture = Texture2D::from_image(&image);
            *slot = Some(Self { image, texture });
        }
        slot.as_mut().expect("just ensured")
    }
}

/// Draws the minimap under the same fog rules as the world view
/// (omniscient while the F1 overlay is up).
pub(crate) fn draw_minimap(game: &crate::game::Scene<'_>) {
    let rect = game.presentation.layout.get().minimap;
    if rect.w <= 0.0 || rect.h <= 0.0 {
        return;
    }
    let scale = rect.w / game.state.map().width() as f32;
    let omniscient = game.presentation.all_seeing();
    let vision = game.my_vision();
    draw_rectangle(
        rect.x - 3.0,
        rect.y - 3.0,
        rect.w + 6.0,
        rect.h + 6.0,
        PANEL,
    );

    let mut layer_slot = game.presentation.minimap_layer.borrow_mut();
    let layer = MinimapLayer::ensure(
        &mut layer_slot,
        game.state.map().width(),
        game.state.map().height(),
    );
    for (pos, tile) in game.state.map().iter() {
        let (explored, visible) = if omniscient {
            (true, true)
        } else {
            (vision.explored(pos), vision.visible(pos))
        };
        let color = if explored {
            // Same memory rule as the world view: live scrap in sight,
            // last-seen scrap under the dim.
            let scrap = if visible {
                tile.scrap
            } else {
                vision.remembered_scrap(pos)
            };
            let base = match (tile.terrain, scrap) {
                (oxide_sim::map::Terrain::Rock, _) => MINI_ROCK,
                (oxide_sim::map::Terrain::Peak, _) => MINI_PEAK,
                (oxide_sim::map::Terrain::Pit, _) => MINI_PIT,
                (_, 0) => MINI_GROUND,
                (_, _) => SCRAP_COLOR,
            };
            if visible {
                // Stamp what is on show. The world renderer stamps only
                // camera-visible tiles, so without this a node re-scouted
                // off-camera would keep fading from its old timestamp.
                if tile.scrap > 0 || tile.wreck > 0 {
                    game.presentation
                        .last_seen
                        .borrow_mut()
                        .insert((pos.x, pos.y), game.presentation.fx_time());
                }
                base
            } else if scrap > 0 {
                // Remembered salvage fades like the world view's but
                // never disappears.
                stale_toward(
                    dim(base),
                    dim(MINI_GROUND),
                    memory_age(game, (pos.x, pos.y)),
                )
            } else {
                dim(base)
            }
        } else {
            MINI_VOID
        };
        layer
            .image
            .set_pixel(pos.x.fit::<u32>(), pos.y.fit::<u32>(), color);
    }
    layer.texture.update(&layer.image);
    // Downscaled tiles (sub-pixel on grand maps) blend; upscaled tiles
    // stay crisp blocks.
    layer.texture.set_filter(if scale < 1.0 {
        FilterMode::Linear
    } else {
        FilterMode::Nearest
    });
    draw_texture_ex(
        &layer.texture,
        rect.x,
        rect.y,
        WHITE,
        DrawTextureParams {
            dest_size: Some(vec2(rect.w, rect.h)),
            ..Default::default()
        },
    );
    drop(layer_slot);

    if !omniscient {
        for ghost in vision.ghosts() {
            let (w, h) = ghost.kind.base_stats().size;
            let age = memory_age(game, (ghost.anchor.x, ghost.anchor.y));
            // Ghosts keep their dimmed seat color as it fades, so
            // allegiance stays readable when the player plans from memory.
            let color = stale_toward(
                dim(mini_entity_color(game, ghost.owner)),
                dim(MINI_GROUND),
                age,
            );
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
            || building.player == game.presentation.human
            || (building.tiles().any(|t| vision.visible(t))
                && game
                    .state
                    .building_apparent(game.presentation.human, building));
        if !seen {
            continue;
        }
        let (w, h) = building.stats().size;
        draw_rectangle(
            rect.x + building.anchor.x as f32 * scale,
            rect.y + building.anchor.y as f32 * scale,
            w as f32 * scale,
            h as f32 * scale,
            mini_entity_color(game, building.player),
        );
    }
    for unit in game.state.units() {
        let seen =
            omniscient || unit.player == game.presentation.human || vision.visible(unit.tile());
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
    let (lo, hi) = game.presentation.camera.world_rect();
    let x = rect.x + lo.x.max(0.0) * scale;
    let y = rect.y + lo.y.max(0.0) * scale;
    let x2 = rect.x + hi.x.min(game.state.map().width() as f32) * scale;
    let y2 = rect.y + hi.y.min(game.state.map().height() as f32) * scale;
    draw_rectangle_lines(x, y, (x2 - x).max(4.0), (y2 - y).max(4.0), 1.5, BONE);

    // Under-attack pulses: an expanding, fading ring at each alert, or
    // under reduced motion a steady marker that fades without expanding.
    for (world, age) in &game.presentation.alerts {
        let center = vec2(rect.x + world.x * scale, rect.y + world.y * scale);
        let (radius, alpha) = if reduced_motion() {
            (5.0, (1.0 - (age / 6.0)).max(0.0))
        } else {
            let pulse = (age * 1.5).fract();
            (
                2.0 + pulse * 10.0,
                (1.0 - pulse) * (1.0 - (age / 6.0)).max(0.0),
            )
        };
        stroke_circle(center, radius, 1.5, Color::new(0.85, 0.32, 0.29, alpha));
    }
}

#[cfg(test)]
mod tests;
