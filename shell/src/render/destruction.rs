//! Rigid casualties, ballistic fragments, and ground-contact effects.

use super::{air_presentation, reduced_motion, seat_identity_tint, unit_draw_scale};
use crate::assets::Sprites;
use crate::game::{EffectKind, Game, UnitBody};
use macroquad::prelude::*;
use oxide_sim::{BuildingKind, ProjectileKind};

fn pixel_rect(center: Vec2, size: Vec2, pixel: f32, color: Color) {
    let origin = (center - size * 0.5) / pixel;
    draw_rectangle(
        origin.x.round() * pixel,
        origin.y.round() * pixel,
        (size.x / pixel).round().max(1.0) * pixel,
        (size.y / pixel).round().max(1.0) * pixel,
        color,
    );
}

fn hash(seed: u32, index: u32) -> u32 {
    seed.wrapping_mul(2_654_435_761)
        .wrapping_add(index.wrapping_mul(40_503))
        .rotate_left(13)
}

fn rotate(v: Vec2, angle: f32) -> Vec2 {
    let (s, c) = angle.sin_cos();
    vec2(v.x * c - v.y * s, v.x * s + v.y * c)
}

fn visible(game: &Game, at: Vec2) -> bool {
    game.all_seeing()
        || game.my_vision().visible(chassis::grid::TilePos::new(
            at.x.floor() as i32,
            at.y.floor() as i32,
        ))
}

pub(super) fn casualty_visible(game: &Game, at: Vec2, body: UnitBody) -> bool {
    body.player == game.human || visible(game, at)
}

fn floor_contact(game: &Game, at: Vec2) -> bool {
    game.state
        .map()
        .tile(chassis::grid::TilePos::new(
            at.x.floor() as i32,
            at.y.floor() as i32,
        ))
        .is_some_and(|tile| tile.terrain != oxide_sim::map::Terrain::Pit)
}

fn ground_mark(center: Vec2, zoom: f32, radius: f32, alpha: f32) {
    let pixel = (zoom / 32.0).max(1.0);
    for i in 0..16 {
        let h = hash(47, i);
        let offset = vec2(
            (h % 100) as f32 / 100.0 - 0.5,
            ((h >> 8) % 100) as f32 / 100.0 - 0.5,
        );
        pixel_rect(
            center + offset * vec2(radius, radius * 0.38) * zoom,
            vec2(radius * (0.16 + (h % 13) as f32 * 0.01), radius * 0.12) * zoom,
            pixel,
            Color::new(0.09, 0.08, 0.065, alpha * 0.32),
        );
    }
}

pub(super) fn draw_hit(sprites: &Sprites, center: Vec2, zoom: f32, radius: f32, progress: f32) {
    let t = progress.clamp(0.0, 1.0);
    let pixel = (zoom / 32.0).max(1.0);
    for i in 0..6 {
        let h = hash(83, i);
        let angle = (h % 628) as f32 / 100.0;
        let reach = radius * zoom * (0.08 + t * 0.48);
        let p = center + vec2(angle.cos(), angle.sin()) * reach;
        pixel_rect(
            p,
            Vec2::splat(zoom * radius * (0.09 + t * 0.08)),
            pixel,
            Color::new(0.23, 0.21, 0.18, (1.0 - t) * 0.5),
        );
    }
    let heat = (1.0 - t * 2.0).max(0.0);
    if heat > 0.0 {
        let source = sprites.burst();
        // Reuse the hot center without the old expanding perimeter ring.
        let core = Rect::new(
            source.x + source.w * 0.375,
            source.y + source.h * 0.375,
            source.w * 0.25,
            source.h * 0.25,
        );
        let size = zoom * radius * (0.22 + t * 0.12);
        sprites.draw(
            center.x - size * 0.5,
            center.y - size * 0.5,
            Color::new(1.0, 0.83, 0.61, heat),
            DrawTextureParams {
                dest_size: Some(Vec2::splat(size)),
                source: Some(core),
                ..Default::default()
            },
        );
    }
}

fn dust(center: Vec2, zoom: f32, spread: f32, age: f32, seed: u32) {
    if !(0.0..0.95).contains(&age) {
        return;
    }
    let t = age / 0.95;
    let fade = (1.0 - t).powi(2);
    let pixel = (zoom / 32.0).max(1.0);
    for i in 0..9 {
        let h = hash(seed, i);
        let angle = (h % 628) as f32 / 100.0;
        let reach = spread * (0.12 + (1.0 - (1.0 - t).powi(3)) * 0.7);
        let p = center + vec2(angle.cos() * reach, angle.sin() * reach * 0.36) * zoom;
        let size = zoom * spread.sqrt() * (0.17 + t * 0.26);
        pixel_rect(
            p + vec2(0.0, size * 0.14),
            vec2(size * 1.3, size * 0.45),
            pixel,
            Color::new(0.08, 0.072, 0.06, fade * 0.40),
        );
        pixel_rect(
            p,
            vec2(size, size * 0.48),
            pixel,
            Color::new(0.29, 0.265, 0.215, fade * 0.70),
        );
        pixel_rect(
            p - vec2(size * 0.12, size * 0.18),
            vec2(size * 0.58, size * 0.36),
            pixel,
            Color::new(0.43, 0.39, 0.31, fade * 0.43),
        );
    }
}

/// Returns planar travel and height. Fragments stay on the floor after landing.
fn fragment_pose(age: f32, flight: f32, reach: f32, height: f32) -> (f32, f32) {
    let t = (age / flight).clamp(0.0, 1.0);
    let skid = ((age - flight) / 0.25).clamp(0.0, 1.0);
    (
        reach * t + reach * 0.12 * (1.0 - (1.0 - skid).powi(2)),
        4.0 * height * t * (1.0 - t),
    )
}

pub(super) fn draw_impact(center: Vec2, zoom: f32, radius: f32, age: f32, payload: ProjectileKind) {
    let missile = payload == ProjectileKind::Missile;
    let reach = radius.max(0.3);
    let pixel = (zoom / 32.0).max(1.0);
    // The flare's foot stays at the point of impact; dust spreads along the floor.
    let hot_life = if missile { 0.24 } else { 0.14 };
    if age < hot_life {
        let heat = (1.0 - age / hot_life).max(0.0);
        let core = zoom * reach * (0.16 + (1.0 - heat) * 0.16);
        for (offset, size, color) in [
            (
                vec2(0.0, 0.03),
                vec2(1.55, 0.48),
                color_u8!(123, 66, 38, 240),
            ),
            (
                vec2(0.12, -0.28),
                vec2(0.85, 1.05),
                color_u8!(203, 116, 48, 255),
            ),
            (
                vec2(0.03, -0.25),
                vec2(0.44, 0.62),
                color_u8!(247, 210, 133, 255),
            ),
        ] {
            pixel_rect(
                center + offset * core,
                size * core,
                pixel,
                Color::new(color.r, color.g, color.b, color.a * heat),
            );
        }
    }
    dust(
        center,
        zoom,
        reach,
        age - 0.025,
        if missile { 71 } else { 19 },
    );
    if reduced_motion() {
        return;
    }
    for i in 0..if missile { 7 } else { 5 } {
        let h = hash(if missile { 31 } else { 13 }, i);
        let angle = (h % 628) as f32 / 100.0;
        let flight = 0.20 + (h % 16) as f32 * 0.01;
        let (travel, height) =
            fragment_pose(age, flight, reach * 0.65, if missile { 0.13 } else { 0.20 });
        let ground = center + vec2(angle.cos() * travel, angle.sin() * travel * 0.55) * zoom;
        let fade = ((1.4 - age) / 0.65).clamp(0.0, 1.0);
        pixel_rect(
            ground + vec2(zoom * 0.035, zoom * 0.045),
            vec2(zoom * 0.095, zoom * 0.035),
            pixel,
            Color::new(0.035, 0.03, 0.025, fade * 0.7),
        );
        let color = if missile && age < flight * 0.55 {
            color_u8!(214, 143, 64, 255)
        } else {
            color_u8!(105, 92, 72, 255)
        };
        pixel_rect(
            ground - vec2(0.0, height * zoom),
            vec2(zoom * 0.07, zoom * 0.045),
            pixel,
            Color::new(color.r, color.g, color.b, fade),
        );
    }
}

#[derive(Clone, Copy, Debug)]
struct PiecePose {
    offset: Vec2,
    rotation: f32,
    shade: f32,
    alpha: f32,
}

fn collapse_piece(index: usize, age: f32, seed: u32) -> PiecePose {
    let h = hash(seed, index as u32);
    let delay = 0.06 + (index % 3) as f32 * 0.035;
    let t = ((age - delay) / 0.34).clamp(0.0, 1.0);
    let impulse = 1.0 - (1.0 - t).powi(2);
    let direction = vec2(
        (h % 101) as f32 / 100.0 - 0.5,
        ((h >> 8) % 101) as f32 / 100.0 - 0.5,
    );
    let retained = index % 3 == 1;
    PiecePose {
        offset: direction * impulse * 0.38 + vec2(0.0, t * t * 0.10),
        rotation: direction.x * impulse * 0.72,
        shade: 1.0 - t * 0.64,
        alpha: if retained {
            ((4.5 - age) / 1.0).clamp(0.0, 1.0)
        } else {
            (1.0 - (age - delay - 0.30) / 0.18).clamp(0.0, 1.0)
        },
    }
}

struct RigidSprite {
    center: Vec2,
    size: Vec2,
    rotation: f32,
    source: Rect,
    accent: Rect,
    tint: Option<Color>,
}

impl RigidSprite {
    fn piece(&self, sprites: &Sprites, part: Rect, pose: PiecePose, zoom: f32) {
        let local_center =
            (vec2(part.x + part.w * 0.5, part.y + part.h * 0.5) - vec2(0.5, 0.5)) * self.size;
        let center = self.center + rotate(local_center, self.rotation) + pose.offset * zoom;
        let size = vec2(part.w, part.h) * self.size;
        for (source, tint) in [(self.source, Some(WHITE)), (self.accent, self.tint)] {
            let Some(tint) = tint else { continue };
            sprites.draw(
                center.x - size.x * 0.5,
                center.y - size.y * 0.5,
                Color::new(
                    tint.r * pose.shade,
                    tint.g * pose.shade,
                    tint.b * pose.shade,
                    pose.alpha,
                ),
                DrawTextureParams {
                    dest_size: Some(size),
                    source: Some(Rect::new(
                        source.x + part.x * source.w,
                        source.y + part.y * source.h,
                        part.w * source.w,
                        part.h * source.h,
                    )),
                    rotation: self.rotation + pose.rotation,
                    ..Default::default()
                },
            );
        }
    }
}

const HULL_PIECES: [Rect; 4] = [
    Rect::new(0.0, 0.0, 0.32, 1.0),
    Rect::new(0.68, 0.0, 0.32, 1.0),
    Rect::new(0.32, 0.0, 0.36, 0.52),
    Rect::new(0.32, 0.52, 0.36, 0.48),
];

fn draw_unit_wreck(
    game: &Game,
    sprites: &Sprites,
    at: Vec2,
    body: UnitBody,
    seed: u32,
    age: f32,
    witnessed: bool,
) {
    if age < 0.0 || !(witnessed || casualty_visible(game, at, body)) || !floor_contact(game, at) {
        return;
    }
    let zoom = game.camera.zoom;
    let center = game.camera.to_screen(at);
    let scale = unit_draw_scale(body.kind);
    let t = if reduced_motion() {
        1.0
    } else {
        (age / 0.35).clamp(0.0, 1.0)
    };
    let alpha = ((2.9 - age) / 0.8).clamp(0.0, 1.0);
    ground_mark(center, zoom, scale * 0.55, alpha * 0.7);
    let hull = RigidSprite {
        center,
        size: Vec2::splat(scale * zoom),
        rotation: body.rotation,
        source: sprites.unit(body.kind, body.faction),
        accent: sprites.unit_accent(body.kind),
        tint: seat_identity_tint(game, body.player),
    };
    for (i, part) in HULL_PIECES.into_iter().enumerate() {
        let h = hash(seed, i as u32);
        let direction = vec2(
            (h % 101) as f32 / 100.0 - 0.5,
            ((h >> 8) % 101) as f32 / 100.0 - 0.5,
        );
        hull.piece(
            sprites,
            part,
            PiecePose {
                offset: direction * (1.0 - (1.0 - t).powi(3)) * 0.35 * scale,
                rotation: direction.x * t * 0.48,
                shade: 1.0 - t * 0.65,
                alpha,
            },
            zoom,
        );
    }
    if body.kind != oxide_sim::UnitKind::Sapper {
        let (radius, payload) = if large_airframe(body.kind) {
            (scale * 0.75, ProjectileKind::Missile)
        } else {
            (scale * 0.46, ProjectileKind::Shell)
        };
        draw_impact(center, zoom, radius, age, payload);
    }
    dust(center, zoom, scale * 0.65, age - 0.04, seed);
}

fn large_airframe(kind: oxide_sim::UnitKind) -> bool {
    matches!(
        kind,
        oxide_sim::UnitKind::Condor | oxide_sim::UnitKind::Moth | oxide_sim::UnitKind::Skyhook
    )
}

const CRASH_TIME: f32 = oxide_sim::stats::AIRCRAFT_CRASH_TICKS as f32 * crate::game::TICK_DT;

#[derive(Clone, Copy, Debug)]
struct CrashPose {
    at: Vec2,
    body_at: Vec2,
    rotation: f32,
}

fn crash_pose(at: Vec2, body: UnitBody, age: f32, reduced: bool) -> CrashPose {
    let t = if reduced {
        1.0
    } else {
        (age / CRASH_TIME).clamp(0.0, 1.0)
    };
    let trajectory = if reduced {
        at
    } else {
        at + body.velocity * CRASH_TIME * (t - 0.2 * t * t)
    };
    let (_, shadow_offset, lift) = air_presentation(body.kind, 1.0);
    let contact = if reduced {
        at
    } else {
        trajectory + shadow_offset
    };
    CrashPose {
        at: contact,
        body_at: if reduced {
            contact
        } else {
            trajectory + shadow_offset * (t * t) - vec2(0.0, lift * (1.0 - t * t))
        },
        rotation: body.rotation,
    }
}

fn fall_pose(
    at: Vec2,
    body: UnitBody,
    age: f32,
    crash: Option<oxide_sim::state::AircraftCrash>,
) -> CrashPose {
    let Some(crash) = crash else {
        return crash_pose(at, body, age, reduced_motion());
    };
    let t = (age / CRASH_TIME).clamp(0.0, 1.0);
    let launch = vec2(crash.launch.x.to_num(), crash.launch.y.to_num());
    let impact = vec2(crash.impact.x.to_num(), crash.impact.y.to_num());
    let trajectory = launch.lerp(impact, (t - 0.2 * t * t) / 0.8);
    let (_, shadow_offset, lift) = air_presentation(body.kind, 1.0);
    CrashPose {
        at: trajectory + shadow_offset * (1.0 - t * t),
        body_at: trajectory - vec2(0.0, lift * (1.0 - t * t)),
        rotation: body.rotation,
    }
}

#[derive(Clone, Copy, Debug)]
struct AirFragment {
    offset: Vec2,
    lift: f32,
    rotation: f32,
    alpha: f32,
}

fn air_fragment(body: UnitBody, seed: u32, index: usize, age: f32) -> AirFragment {
    let h = hash(seed, index as u32);
    let flight = 0.42 + (h % 20) as f32 * 0.01;
    let t = (age / flight).clamp(0.0, 1.0);
    let direction = rotate(
        vec2(
            if index == 0 {
                -1.0
            } else if index == 1 {
                1.0
            } else {
                0.25
            },
            if index < 2 {
                0.0
            } else if index == 2 {
                -1.0
            } else {
                1.0
            },
        ),
        body.rotation,
    );
    let (reach, loft) = fragment_pose(age, flight, 0.38, 0.13);
    AirFragment {
        offset: direction * (0.07 + reach) + body.velocity * flight * (t - 0.5 * t * t),
        lift: air_presentation(body.kind, 1.0).2 * (1.0 - t * t) + loft,
        rotation: ((h % 101) as f32 / 100.0 - 0.5) * t * 1.3,
        alpha: ((1.65 - age) / 0.7).clamp(0.0, 1.0),
    }
}

fn draw_air_fragments(
    game: &Game,
    sprites: &Sprites,
    at: Vec2,
    body: UnitBody,
    seed: u32,
    age: f32,
    grounded: bool,
) {
    let zoom = game.camera.zoom;
    for (i, part) in HULL_PIECES.into_iter().enumerate() {
        let pose = air_fragment(body, seed, i, age);
        if (pose.lift == 0.0) != grounded
            || !casualty_visible(game, at + pose.offset, body)
            || (grounded && !floor_contact(game, at + pose.offset))
        {
            continue;
        }
        let hull = RigidSprite {
            center: game.camera.to_screen(at + pose.offset) - vec2(0.0, pose.lift * zoom),
            size: Vec2::splat(unit_draw_scale(body.kind) * zoom),
            rotation: body.rotation,
            source: sprites.unit(body.kind, body.faction),
            accent: sprites.unit_accent(body.kind),
            tint: seat_identity_tint(game, body.player),
        };
        hull.piece(
            sprites,
            part,
            PiecePose {
                offset: Vec2::ZERO,
                rotation: pose.rotation,
                shade: 1.0 - (age / 0.4).clamp(0.0, 1.0) * 0.65,
                alpha: pose.alpha,
            },
            zoom,
        );
    }
}

fn draw_airburst(center: Vec2, zoom: f32, scale: f32, age: f32) {
    let t = (age / 0.32).clamp(0.0, 1.0);
    let pixel = (zoom / 32.0).max(1.0);
    let size = zoom * scale;
    for i in 0..6 {
        let h = hash(103, i);
        let angle = (h % 628) as f32 / 100.0;
        let p = center + vec2(angle.cos(), angle.sin()) * size * (0.12 + t * 0.45);
        pixel_rect(
            p,
            vec2(0.17, 0.12) * size * (1.0 + t),
            pixel,
            Color::new(0.24, 0.22, 0.19, (1.0 - t) * 0.60),
        );
    }
    let heat = (1.0 - t * 1.8).max(0.0);
    for (offset, shape, color) in [
        (
            vec2(-0.04, 0.04),
            vec2(0.56, 0.28),
            color_u8!(156, 77, 35, 255),
        ),
        (
            vec2(0.04, -0.03),
            vec2(0.32, 0.43),
            color_u8!(225, 147, 61, 255),
        ),
        (
            vec2(-0.02, -0.05),
            vec2(0.19, 0.22),
            color_u8!(255, 226, 157, 255),
        ),
    ] {
        pixel_rect(
            center + offset * size,
            shape * size,
            pixel,
            Color::new(color.r, color.g, color.b, heat),
        );
    }
}

pub(super) fn draw_falling(
    game: &Game,
    sprites: &Sprites,
    at: Vec2,
    body: UnitBody,
    seed: u32,
    age: f32,
    crash: Option<oxide_sim::state::AircraftCrash>,
) {
    if !large_airframe(body.kind) {
        if !reduced_motion() {
            draw_air_fragments(game, sprites, at, body, seed, age, false);
        }
        if casualty_visible(game, at, body) && age < 0.32 {
            let center = game.camera.to_screen(at)
                - vec2(0.0, air_presentation(body.kind, game.camera.zoom).2);
            draw_airburst(center, game.camera.zoom, unit_draw_scale(body.kind), age);
        }
        return;
    }
    if age >= CRASH_TIME || (reduced_motion() && crash.is_none()) {
        return;
    }
    let pose = fall_pose(at, body, age, crash);
    if !casualty_visible(game, pose.at, body) {
        return;
    }
    let zoom = game.camera.zoom;
    let ground = game.camera.to_screen(pose.at);
    let size = unit_draw_scale(body.kind) * zoom;
    let t = age / CRASH_TIME;
    let (shadow_size, _, _) = air_presentation(body.kind, zoom);
    if floor_contact(game, pose.at) {
        sprites.draw(
            ground.x - shadow_size.x * 0.5,
            ground.y - shadow_size.y * 0.5,
            WHITE,
            DrawTextureParams {
                dest_size: Some(shadow_size),
                source: Some(sprites.air_shadow()),
                ..Default::default()
            },
        );
    }
    let hull = RigidSprite {
        center: game.camera.to_screen(pose.body_at),
        size: Vec2::splat(size),
        rotation: pose.rotation,
        source: sprites.unit(body.kind, body.faction),
        accent: sprites.unit_accent(body.kind),
        tint: seat_identity_tint(game, body.player),
    };
    hull.piece(
        sprites,
        Rect::new(0.0, 0.0, 1.0, 1.0),
        PiecePose {
            offset: Vec2::ZERO,
            rotation: 0.0,
            shade: 1.0 - t * 0.3,
            alpha: 1.0,
        },
        zoom,
    );
    if reduced_motion() {
        return;
    }
    // An engine trail stays attached to earlier positions along the final coast.
    for i in 1..4 {
        let earlier = (age - i as f32 * 0.035).max(0.0);
        let trail = fall_pose(at, body, earlier, crash);
        let p = game.camera.to_screen(trail.body_at);
        pixel_rect(
            p,
            vec2(size * 0.11, size * 0.08),
            (zoom / 32.0).max(1.0),
            Color::new(0.12, 0.115, 0.105, (0.32 - i as f32 * 0.06) * t),
        );
    }
}

pub(super) fn draw_ground_effects(game: &Game, sprites: &Sprites) {
    let zoom = game.camera.zoom;
    for effect in &game.fx {
        match effect.kind {
            EffectKind::Impact { at, radius, .. }
                if visible(game, at) && floor_contact(game, at) =>
            {
                ground_mark(
                    game.camera.to_screen(at),
                    zoom,
                    radius * 0.68,
                    ((1.4 - effect.age) / 0.7).clamp(0.0, 1.0) * 0.70,
                );
            }
            EffectKind::Debris { at, body, seed } => {
                let delay = if body.kind == oxide_sim::UnitKind::Sapper {
                    0.1
                } else {
                    0.0
                };
                draw_unit_wreck(game, sprites, at, body, seed, effect.age - delay, false);
            }
            EffectKind::Falling {
                at,
                body,
                seed,
                crash,
                impact_witnessed,
            } => {
                if !large_airframe(body.kind) {
                    if !reduced_motion() {
                        draw_air_fragments(game, sprites, at, body, seed, effect.age, true);
                    }
                    continue;
                }
                let delay = if reduced_motion() && crash.is_none() {
                    0.0
                } else {
                    CRASH_TIME
                };
                let pose = fall_pose(at, body, delay, crash);
                let age = effect.age_at(game.state.current_tick(), game.tick_fraction());
                if crash.is_some_and(|crash| game.state.current_tick() <= crash.arrival) {
                    continue;
                }
                draw_unit_wreck(
                    game,
                    sprites,
                    pose.at,
                    UnitBody {
                        rotation: pose.rotation,
                        ..body
                    },
                    seed,
                    age - delay,
                    impact_witnessed,
                );
            }
            EffectKind::Collapse { at, body, seed }
                if visible(game, at) || body.player == game.human =>
            {
                let center = game.camera.to_screen(at);
                let (w, h) = body.kind.base_stats().size;
                let size = vec2(w as f32, h as f32) * zoom;
                let age = if reduced_motion() {
                    effect.age + 0.5
                } else {
                    effect.age
                };
                ground_mark(
                    center,
                    zoom,
                    w as f32 * 0.66,
                    ((4.5 - effect.age) / 1.0).clamp(0.0, 1.0) * 0.7,
                );
                let structure = RigidSprite {
                    center,
                    size,
                    rotation: 0.0,
                    source: sprites.building_tiered(body.kind, body.tier, body.faction),
                    accent: sprites.building_tiered_accent(body.kind, body.tier),
                    tint: seat_identity_tint(game, body.player),
                };
                for column in 0..3 {
                    let x = [0.0, 0.25, 0.75][column];
                    let width = [0.25, 0.5, 0.25][column];
                    for row in 0..3 {
                        let index = column * 3 + row;
                        structure.piece(
                            sprites,
                            Rect::new(x, row as f32 / 3.0, width, 1.0 / 3.0),
                            collapse_piece(index, age, seed),
                            zoom,
                        );
                    }
                }
                if let Some(source) = sprites.defense_mount(body.kind, body.tier, body.faction) {
                    let mount = RigidSprite {
                        rotation: body.rotation,
                        source,
                        accent: sprites
                            .defense_mount_accent(body.kind, body.tier)
                            .expect("defense accent accompanies its mount"),
                        ..structure
                    };
                    for (i, part) in HULL_PIECES.into_iter().enumerate() {
                        mount.piece(sprites, part, collapse_piece(i + 9, age, seed), zoom);
                    }
                }
                if age > 0.28 {
                    let fade = ((4.5 - effect.age) / 1.0).clamp(0.0, 1.0);
                    for i in 0..3 {
                        let h = hash(seed, i);
                        let p = center
                            + vec2(
                                (h % 101) as f32 / 100.0 - 0.5,
                                ((h >> 8) % 101) as f32 / 100.0 - 0.5,
                            ) * size
                                * 0.5;
                        let fragment = zoom * 0.35;
                        sprites.draw(
                            p.x - fragment * 0.5,
                            p.y - fragment * 0.5,
                            Color::new(0.52, 0.48, 0.42, fade),
                            DrawTextureParams {
                                dest_size: Some(Vec2::splat(fragment)),
                                source: Some(sprites.debris(i as usize)),
                                ..Default::default()
                            },
                        );
                    }
                }
                let spread = if body.kind == BuildingKind::Foundry {
                    1.35
                } else {
                    0.95
                };
                dust(center, zoom, w as f32 * 0.6, age - 0.22, seed);
                draw_impact(center, zoom, spread, age, ProjectileKind::Shell);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduled_airframe_contacts_the_damage_point_with_its_full_level_pose() {
        for kind in [
            oxide_sim::UnitKind::Condor,
            oxide_sim::UnitKind::Moth,
            oxide_sim::UnitKind::Skyhook,
        ] {
            let crash = oxide_sim::state::AircraftCrash {
                unit: oxide_sim::UnitId(0),
                player: oxide_sim::PlayerId(0),
                kind,
                heading: 0,
                launch: chassis::grid::TilePos::new(8, 8).center(),
                impact: chassis::grid::TilePos::new(9, 8).center(),
                started: 10,
                arrival: 23,
            };
            let body = UnitBody {
                kind,
                player: crash.player,
                faction: oxide_sim::Faction::Ferrous,
                rotation: 1.2,
                velocity: Vec2::ZERO,
            };
            let start = fall_pose(Vec2::ZERO, body, 0.0, Some(crash));
            let halfway = fall_pose(Vec2::ZERO, body, CRASH_TIME / 2.0, Some(crash));
            let contact = fall_pose(Vec2::ZERO, body, CRASH_TIME, Some(crash));
            assert!(start.body_at.x < halfway.body_at.x && halfway.body_at.x < contact.body_at.x);
            assert_eq!(contact.at, vec2(9.5, 8.5));
            assert_eq!(contact.body_at, contact.at);
            assert_eq!(start.rotation, contact.rotation);
        }
    }

    #[test]
    fn own_crash_remains_visible_when_the_casualty_was_the_last_vision_source() {
        let mut scenario = oxide_sim::Scenario::skirmish();
        scenario.map = vec![".".repeat(40); 26];
        scenario.map[3].replace_range(2..3, "1");
        scenario.map[3].replace_range(35..36, "2");
        for player in &mut scenario.players {
            player.bot_config = None;
        }
        scenario.units = vec![oxide_sim::scenario::UnitSpec {
            player: 0,
            kind: oxide_sim::UnitKind::Condor,
            x: 25,
            y: 20,
        }];
        for y in [17, 20, 23] {
            scenario.units.push(oxide_sim::scenario::UnitSpec {
                player: 1,
                kind: oxide_sim::UnitKind::Flakhound,
                x: 28,
                y,
            });
        }
        let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
        for _ in 0..600 {
            game.present_ticks(1);
            if let Some((at, body)) = game.fx.iter().find_map(|effect| match effect.kind {
                EffectKind::Falling { at, body, .. } => Some((at, body)),
                _ => None,
            }) {
                assert!(!visible(&game, at));
                assert!(casualty_visible(&game, at, body));
                assert!(!casualty_visible(
                    &game,
                    at,
                    UnitBody {
                        player: oxide_sim::PlayerId(1),
                        ..body
                    },
                ));
                return;
            }
        }
        panic!("the flak line did not destroy the aircraft");
    }

    #[test]
    fn fragments_land_once_and_stop_without_bouncing() {
        let at_contact = fragment_pose(0.3, 0.3, 0.7, 0.2);
        let after_skid = fragment_pose(0.55, 0.3, 0.7, 0.2);
        assert_eq!(at_contact.1, 0.0);
        assert_eq!(after_skid.1, 0.0);
        assert_eq!(after_skid, fragment_pose(8.0, 0.3, 0.7, 0.2));
        assert!(after_skid.0 > at_contact.0);
    }

    #[test]
    fn aircraft_contact_preserves_heading_and_stops_the_coast() {
        let body = UnitBody {
            kind: oxide_sim::UnitKind::Condor,
            player: oxide_sim::PlayerId(0),
            faction: oxide_sim::Faction::Ferrous,
            rotation: 2.1,
            velocity: vec2(-2.0, 1.0),
        };
        let start = crash_pose(vec2(8.0, 8.0), body, 0.0, false);
        let contact = crash_pose(vec2(8.0, 8.0), body, CRASH_TIME, false);
        let settled = crash_pose(vec2(8.0, 8.0), body, 3.0, false);
        assert_eq!(start.rotation, body.rotation);
        assert!(start.body_at.y < start.at.y);
        assert_eq!(contact.body_at, contact.at);
        assert_eq!(contact.at, settled.at);
        assert_eq!(contact.rotation, body.rotation);
        assert!(contact.at.x < start.at.x && contact.at.y > start.at.y);
        assert_eq!(crash_pose(start.at, body, 0.0, true).at, start.at);
    }

    #[test]
    fn a_hovering_airframe_drops_level_to_its_existing_shadow() {
        let body = UnitBody {
            kind: oxide_sim::UnitKind::Skyhook,
            player: oxide_sim::PlayerId(0),
            faction: oxide_sim::Faction::Ferrous,
            rotation: 0.0,
            velocity: Vec2::ZERO,
        };
        let start = crash_pose(Vec2::ZERO, body, 0.0, false);
        let middle = crash_pose(Vec2::ZERO, body, CRASH_TIME * 0.5, false);
        let contact = crash_pose(Vec2::ZERO, body, CRASH_TIME, false);
        let (_, offset, lift) = air_presentation(body.kind, 1.0);
        assert_eq!(start.body_at, vec2(0.0, -lift));
        assert_eq!(start.at, offset);
        assert_eq!(contact.at, start.at);
        assert_eq!(contact.body_at, contact.at);
        assert!(middle.body_at.y > start.body_at.y && middle.body_at.y < contact.body_at.y);
        assert_eq!(middle.rotation, start.rotation);
        for kind in [
            oxide_sim::UnitKind::Condor,
            oxide_sim::UnitKind::Moth,
            oxide_sim::UnitKind::Skyhook,
        ] {
            assert!(large_airframe(kind));
        }
        for kind in [
            oxide_sim::UnitKind::Kestrel,
            oxide_sim::UnitKind::Buzzard,
            oxide_sim::UnitKind::Wisp,
        ] {
            assert!(!large_airframe(kind));
        }
    }

    #[test]
    fn small_airframe_breaks_above_ground_then_its_fragments_settle() {
        let body = UnitBody {
            kind: oxide_sim::UnitKind::Kestrel,
            player: oxide_sim::PlayerId(0),
            faction: oxide_sim::Faction::Ferrous,
            rotation: 0.0,
            velocity: vec2(1.0, 0.0),
        };
        let left = air_fragment(body, 7, 0, 0.0);
        let right = air_fragment(body, 7, 1, 0.0);
        assert!(left.lift > 0.0 && right.lift > 0.0);
        assert!(left.offset.x < right.offset.x);
        for i in 0..4 {
            let landed = air_fragment(body, 7, i, 1.0);
            let expired = air_fragment(body, 7, i, 2.0);
            assert_eq!(landed.lift, 0.0);
            assert_eq!(landed.offset, expired.offset);
            assert_eq!(landed.rotation, expired.rotation);
            assert_eq!(expired.alpha, 0.0);
        }
    }

    #[test]
    fn structural_fragments_stop_rotating_and_leave_gaps() {
        let retained = collapse_piece(1, 0.8, 7);
        let late = collapse_piece(1, 3.0, 7);
        assert_eq!(retained.offset, late.offset);
        assert_eq!(retained.rotation, late.rotation);
        assert!(retained.rotation.abs() < 0.4);
        assert!(retained.alpha > 0.0);
        assert_eq!(collapse_piece(0, 0.8, 7).alpha, 0.0);
    }
}
