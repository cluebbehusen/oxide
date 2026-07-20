//! Software rendering of sim state — the golden-image workhorse.
//!
//! tiny-skia rasterizes on the CPU with fixed inputs, so the same state
//! produces the same PNG bytes on every machine: golden tests compare
//! byte-for-byte, no GPU, no window, no tolerance tuning. The style is a
//! deliberately plain diagram (flat tiles, circles, hp bars) — the shell owns
//! looking good; this owns being comparable.

use anyhow::{Context, Result};
use oxide_sim::map::Terrain;
use oxide_sim::{Faction, State, UnitKind};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Shader, Transform};

/// Pixels per tile.
pub const TILE_PX: f32 = 12.0;

fn rgb(hex: u32) -> Color {
    Color::from_rgba8(
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
        0xff,
    )
}

const GROUND: u32 = 0x232329;
const ROCK: u32 = 0x52525E;
const SCRAP_FULL: u32 = 0xD9A441;
const SCRAP_LOW: u32 = 0x8C6A2F;
const HP_BACK: u32 = 0x141418;
const HP_FRONT: u32 = 0xE8E4D8;

fn faction_color(faction: Faction) -> u32 {
    match faction {
        Faction::Ferrous => 0xC4573B,
        Faction::Cupric => 0x3F9482,
    }
}

fn darken(hex: u32) -> u32 {
    let (r, g, b) = ((hex >> 16) & 0xff, (hex >> 8) & 0xff, hex & 0xff);
    ((r * 3 / 5) << 16) | ((g * 3 / 5) << 8) | (b * 3 / 5)
}

/// Solid fill. Anti-aliasing stays off for axis-aligned rects — it adds
/// nothing there and tiny-skia's AA hairline path asserts on sub-pixel
/// spans (which hp bars produce constantly).
fn solid(color: Color) -> Paint<'static> {
    Paint {
        shader: Shader::SolidColor(color),
        anti_alias: false,
        ..Paint::default()
    }
}

fn fill_rect(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, color: u32) {
    if let Some(rect) = Rect::from_xywh(x, y, w, h) {
        pixmap.fill_rect(rect, &solid(rgb(color)), Transform::identity(), None);
    }
}

fn fill_circle(pixmap: &mut Pixmap, cx: f32, cy: f32, r: f32, color: u32) {
    let mut pb = PathBuilder::new();
    pb.push_circle(cx, cy, r);
    if let Some(path) = pb.finish() {
        let mut paint = solid(rgb(color));
        paint.anti_alias = true;
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// Draws `state` to a fresh pixmap at [`TILE_PX`] resolution.
pub fn render_state(state: &State) -> Pixmap {
    let width = (state.map().width() as f32 * TILE_PX) as u32;
    let height = (state.map().height() as f32 * TILE_PX) as u32;
    let mut pixmap = Pixmap::new(width, height).expect("nonzero map dimensions");
    pixmap.fill(rgb(GROUND));

    for (pos, tile) in state.map().iter() {
        let (x, y) = (pos.x as f32 * TILE_PX, pos.y as f32 * TILE_PX);
        match (tile.terrain, tile.scrap) {
            (Terrain::Rock, _) => fill_rect(&mut pixmap, x, y, TILE_PX, TILE_PX, ROCK),
            // Rubble: a faint lightening so the goldens register it.
            (Terrain::Ground, 0) if tile.cosmetic == 1 => {
                fill_rect(&mut pixmap, x, y, TILE_PX, TILE_PX, 0x2C2C34);
            }
            (Terrain::Ground, 0) => {}
            (Terrain::Ground, scrap) => {
                // Nodes shrink and dim as they deplete; rich nodes render
                // saturated at full size.
                let fraction = (scrap as f32 / oxide_sim::stats::SCRAP_NODE_AMOUNT as f32).min(1.0);
                let color = if fraction > 0.5 {
                    SCRAP_FULL
                } else {
                    SCRAP_LOW
                };
                let inset = TILE_PX * (0.15 + 0.25 * (1.0 - fraction));
                fill_rect(
                    &mut pixmap,
                    x + inset,
                    y + inset,
                    TILE_PX - 2.0 * inset,
                    TILE_PX - 2.0 * inset,
                    color,
                );
            }
        }
    }

    for building in state.buildings() {
        let color = faction_color(state.player(building.player).faction);
        let (w, h) = building.kind.stats().size;
        let (x, y) = (
            building.anchor.x as f32 * TILE_PX,
            building.anchor.y as f32 * TILE_PX,
        );
        let (pw, ph) = (w as f32 * TILE_PX, h as f32 * TILE_PX);
        fill_rect(&mut pixmap, x, y, pw, ph, darken(color));
        // Unfinished sites show only their dark frame — scaffolding.
        if building.built {
            fill_rect(&mut pixmap, x + 2.0, y + 2.0, pw - 4.0, ph - 4.0, color);
        }
        draw_hp_bar(
            &mut pixmap,
            x,
            y - 4.0,
            pw,
            building.hp,
            building.kind.stats().max_hp,
        );
    }

    for unit in state.units() {
        let color = faction_color(state.player(unit.player).faction);
        let cx = unit.pos.x.to_num::<f32>() * TILE_PX;
        let cy = unit.pos.y.to_num::<f32>() * TILE_PX;
        let r = unit.kind.stats().radius.to_num::<f32>() * TILE_PX;
        fill_circle(&mut pixmap, cx, cy, r + 1.0, darken(color));
        fill_circle(&mut pixmap, cx, cy, r, color);
        if unit.kind == UnitKind::Harvester && unit.carrying > 0 {
            fill_circle(&mut pixmap, cx, cy, r * 0.4, SCRAP_FULL);
        }
        let max_hp = unit.kind.stats().max_hp;
        if unit.hp < max_hp {
            draw_hp_bar(&mut pixmap, cx - r, cy - r - 4.0, 2.0 * r, unit.hp, max_hp);
        }
    }
    pixmap
}

fn draw_hp_bar(pixmap: &mut Pixmap, x: f32, y: f32, w: f32, hp: u32, max_hp: u32) {
    fill_rect(pixmap, x, y, w, 2.0, HP_BACK);
    let fraction = hp as f32 / max_hp as f32;
    fill_rect(pixmap, x, y, w * fraction, 2.0, HP_FRONT);
}

/// Renders straight to PNG bytes (what golden tests compare).
pub fn png_bytes(state: &State) -> Result<Vec<u8>> {
    render_state(state).encode_png().context("encoding png")
}

/// Renders to a PNG file.
pub fn save_png(state: &State, path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    render_state(state)
        .save_png(path)
        .with_context(|| format!("writing {}", path.display()))
}
