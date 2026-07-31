//! Drawing: map, entities, effects, HUD, debug overlay.
//!
//! Split by layer (0.10 file diet): submodules own the minimap and,
//! as the split continues, the panel, chrome, and world layers.
//!
//! Reads the sim, never writes it. Unit positions interpolate between the
//! previous and current tick so 20 sim ticks per second still looks like
//! 60fps motion.

use crate::assets::Sprites;
static COLORBLIND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Colorblind accents: swap allegiance-critical indicator colors for a
/// deutan/protan-safe orange-vs-blue pair. Sprites keep their art —
/// this governs the signals that must never be ambiguous (minimap
/// dots, alert pulses, allegiance tints).
pub fn set_colorblind(on: bool) {
    COLORBLIND.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn colorblind() -> bool {
    COLORBLIND.load(std::sync::atomic::Ordering::Relaxed)
}

/// The faction's indicator accent — the one allegiance color every
/// signal derives from, colorblind-aware.
pub fn faction_accent(faction: oxide_sim::Faction) -> Color {
    match (faction, colorblind()) {
        (oxide_sim::Faction::Ferrous, false) => color_u8!(196, 87, 59, 255),
        (oxide_sim::Faction::Cupric, false) => color_u8!(63, 148, 130, 255),
        // The safe pair: warm orange vs cool blue reads under deutan,
        // protan, and tritan alike.
        (oxide_sim::Faction::Ferrous, true) => color_u8!(230, 120, 30, 255),
        (oxide_sim::Faction::Cupric, true) => color_u8!(70, 120, 235, 255),
    }
}

/// What allegiance signal an entity owes the human viewer. Factions
/// are two seats' worth of identity on team maps — Compass Grand pits
/// the Ferrous human against a Ferrous seat — so the renderers key
/// their friend-or-foe cues off this, never off sprite color alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AllegianceCue {
    /// The viewer's own machines: no ring, plain faction color.
    Mine,
    /// Friendly, not yours: the whitened ring / minimap lift.
    Ally,
    /// A hostile wearing the viewer's own faction: the dark ring /
    /// minimap press — luminance says foe where tint cannot.
    HostileTwin,
    /// An ordinary hostile: faction color already reads as enemy.
    Hostile,
}

pub(crate) fn allegiance_cue(
    game: &crate::game::Game,
    owner: oxide_sim::PlayerId,
) -> AllegianceCue {
    if owner == game.human {
        return AllegianceCue::Mine;
    }
    if !game.state.hostile(game.human, owner) {
        return AllegianceCue::Ally;
    }
    if game.state.player(owner).faction == game.state.player(game.human).faction {
        return AllegianceCue::HostileTwin;
    }
    AllegianceCue::Hostile
}

/// The base allegiance hue beneath per-seat identity: your machines keep
/// pure faction art (`None`), allies occupy a cool family, and hostiles a
/// warm family. Colorblind mode swaps the allied base to bone, preserving a
/// luminance split between friend and foe before seat variation is applied.
pub(crate) fn allegiance_tint(cue: AllegianceCue) -> Option<Color> {
    match (cue, colorblind()) {
        (AllegianceCue::Mine, _) => None,
        (AllegianceCue::Ally, false) => Some(color_u8!(100, 160, 245, 255)),
        (AllegianceCue::Ally, true) => Some(color_u8!(238, 234, 222, 255)),
        (AllegianceCue::Hostile | AllegianceCue::HostileTwin, _) => {
            Some(color_u8!(228, 44, 58, 255))
        }
    }
}

/// A stable seat accent inside the stronger allegiance vocabulary.
///
/// Allies stay in a cool family and hostiles stay in a warm family,
/// but seats within either family receive distinct accents in seat order.
/// The underlying silhouette, allegiance ring, and friend/foe family still
/// carry meaning when hue cannot; the per-seat tint is an identity aid, not
/// the only allegiance signal.
pub(crate) fn seat_identity_color(game: &crate::game::Game, owner: oxide_sim::PlayerId) -> Color {
    let cue = allegiance_cue(game, owner);
    if cue == AllegianceCue::Mine {
        return faction_accent(game.state.player(owner).faction);
    }
    let semantic = allegiance_tint(cue).expect("non-own allegiance has a semantic tint");
    let rank = game
        .state
        .players()
        .iter()
        .enumerate()
        .map(|(seat, _)| oxide_sim::PlayerId(seat as u8))
        .filter(|seat| {
            let other = allegiance_cue(game, *seat);
            matches!(
                (cue, other),
                (AllegianceCue::Ally, AllegianceCue::Ally)
                    | (
                        AllegianceCue::Hostile | AllegianceCue::HostileTwin,
                        AllegianceCue::Hostile | AllegianceCue::HostileTwin
                    )
            )
        })
        .position(|seat| seat == owner)
        .unwrap_or(0);
    let allies = if colorblind() {
        [
            semantic,
            color_u8!(112, 184, 238, 255),
            color_u8!(181, 158, 232, 255),
            color_u8!(145, 207, 190, 255),
            color_u8!(107, 148, 224, 255),
            color_u8!(137, 207, 229, 255),
            color_u8!(200, 181, 239, 255),
            color_u8!(111, 188, 174, 255),
        ]
    } else {
        [
            semantic,
            color_u8!(68, 190, 205, 255),
            color_u8!(165, 139, 235, 255),
            color_u8!(132, 201, 170, 255),
            color_u8!(64, 119, 221, 255),
            color_u8!(113, 203, 239, 255),
            color_u8!(196, 162, 242, 255),
            color_u8!(72, 174, 157, 255),
        ]
    };
    let hostiles = if colorblind() {
        [
            color_u8!(211, 65, 60, 255),
            color_u8!(232, 128, 35, 255),
            color_u8!(173, 72, 125, 255),
            color_u8!(151, 87, 61, 255),
            color_u8!(242, 84, 31, 255),
            color_u8!(207, 99, 104, 255),
            color_u8!(190, 136, 43, 255),
            color_u8!(139, 49, 82, 255),
        ]
    } else {
        [
            semantic,
            color_u8!(232, 105, 42, 255),
            color_u8!(199, 66, 132, 255),
            color_u8!(172, 83, 57, 255),
            color_u8!(246, 73, 24, 255),
            color_u8!(210, 85, 111, 255),
            color_u8!(196, 124, 37, 255),
            color_u8!(154, 50, 85, 255),
        ]
    };
    match cue {
        AllegianceCue::Ally => allies[rank],
        AllegianceCue::Hostile | AllegianceCue::HostileTwin => hostiles[rank],
        AllegianceCue::Mine => unreachable!("mine returned above"),
    }
}

/// The seat-aware sprite/minimap accent. Own machines keep their faction
/// art; every other seat receives its stable ally- or enemy-family tint.
pub(crate) fn seat_identity_tint(
    game: &crate::game::Game,
    owner: oxide_sim::PlayerId,
) -> Option<Color> {
    (owner != game.human).then(|| seat_identity_color(game, owner))
}

/// How faded a memory draws after `age` seconds unseen: 0 fresh,
/// climbing to a 0.55 fade over ninety seconds. Memories never vanish
/// — the player recorded them honestly — they just stop pretending to
/// be news.
pub fn staleness_fade(age: f32) -> f32 {
    (age / 90.0).clamp(0.0, 1.0) * 0.55
}

mod chrome;
pub(crate) mod entities;
mod minimap;
mod motion;
mod panel_draw;
mod world;
use chrome::*;
use entities::*;
pub use minimap::*;
use panel_draw::*;
use world::*;

use crate::game::{EffectKind, Game};
use crate::input::InputState;
use chassis::grid::TilePos;
use macroquad::prelude::*;
use oxide_sim::UnitKind;
use oxide_sim::stats::SCRAP_NODE_AMOUNT;

pub(crate) use crate::theme::{
    SURFACE_CARD, TEXT_BODY, TEXT_DISABLED, TEXT_PRIMARY, TEXT_SECONDARY,
};

pub(crate) const OUTSIDE: Color = color_u8!(20, 20, 25, 255);
// World decoration (selection rings, rally poles, breadcrumbs) keeps
// its own bone pair: the text tiers in crate::theme answer for
// legibility, and raising them must never thicken the world's weight.
const BONE: Color = color_u8!(232, 228, 216, 255);
const BONE_FAINT: Color = color_u8!(232, 228, 216, 90);
const SCRAP_COLOR: Color = crate::theme::TEXT_ACCENT;
const HP_BACK: Color = color_u8!(20, 20, 24, 220);
const DANGER: Color = crate::theme::TEXT_DANGER;
const PANEL: Color = crate::theme::SURFACE_PANEL;

fn combat_icon_color(icon: crate::panel::CombatIcon) -> Color {
    use crate::panel::CombatIcon;
    match icon {
        CombatIcon::Weapon => Color::new(0.85, 0.32, 0.29, 0.86),
        CombatIcon::AirWeapon => Color::new(0.38, 0.70, 0.95, 0.90),
        CombatIcon::DeadZone => Color::new(1.0, 0.68, 0.18, 0.92),
        CombatIcon::Vision => Color::new(0.63, 0.77, 0.94, 0.86),
        CombatIcon::Radar => Color::new(0.22, 0.76, 0.72, 0.90),
        CombatIcon::Repair => Color::new(0.38, 0.82, 0.45, 0.90),
    }
}

fn draw_combat_icon(
    center: Vec2,
    radius: f32,
    icon: crate::panel::CombatIcon,
    color: Color,
    plate: bool,
) {
    use crate::panel::CombatIcon;
    let radius = radius.max(2.0);
    let stroke = (radius * 0.18).clamp(1.0, 2.0);
    if plate {
        draw_circle(
            center.x,
            center.y,
            radius * 1.34,
            Color::new(0.045, 0.045, 0.060, 0.88),
        );
    }
    match icon {
        CombatIcon::Weapon | CombatIcon::DeadZone => {
            draw_circle_lines(center.x, center.y, radius * 0.58, stroke, color);
            draw_line(
                center.x - radius,
                center.y,
                center.x - radius * 0.38,
                center.y,
                stroke,
                color,
            );
            draw_line(
                center.x + radius * 0.38,
                center.y,
                center.x + radius,
                center.y,
                stroke,
                color,
            );
            draw_line(
                center.x,
                center.y - radius,
                center.x,
                center.y - radius * 0.38,
                stroke,
                color,
            );
            draw_line(
                center.x,
                center.y + radius * 0.38,
                center.x,
                center.y + radius,
                stroke,
                color,
            );
            if icon == CombatIcon::DeadZone {
                draw_line(
                    center.x - radius * 0.78,
                    center.y + radius * 0.78,
                    center.x + radius * 0.78,
                    center.y - radius * 0.78,
                    stroke * 1.2,
                    color,
                );
            }
        }
        CombatIcon::AirWeapon => {
            // A top-down aircraft inside four targeting brackets. The
            // aircraft names the domain; the brackets make this an attack
            // reach mark rather than a place the selected unit can fly.
            let corner = radius * 0.34;
            for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                let x = center.x + sx * radius;
                let y = center.y + sy * radius;
                draw_line(x, y, x - sx * corner, y, stroke, color);
                draw_line(x, y, x, y - sy * corner, stroke, color);
            }
            let aircraft = radius * 0.66;
            draw_line(
                center.x,
                center.y - aircraft,
                center.x,
                center.y + aircraft * 0.82,
                stroke,
                color,
            );
            draw_line(
                center.x,
                center.y - aircraft * 0.18,
                center.x - aircraft,
                center.y + aircraft * 0.34,
                stroke,
                color,
            );
            draw_line(
                center.x,
                center.y - aircraft * 0.18,
                center.x + aircraft,
                center.y + aircraft * 0.34,
                stroke,
                color,
            );
            draw_line(
                center.x,
                center.y + aircraft * 0.48,
                center.x - aircraft * 0.48,
                center.y + aircraft * 0.82,
                stroke,
                color,
            );
            draw_line(
                center.x,
                center.y + aircraft * 0.48,
                center.x + aircraft * 0.48,
                center.y + aircraft * 0.82,
                stroke,
                color,
            );
        }
        CombatIcon::Vision => {
            let left = vec2(center.x - radius, center.y);
            let right = vec2(center.x + radius, center.y);
            let upper = vec2(center.x, center.y - radius * 0.62);
            let lower = vec2(center.x, center.y + radius * 0.62);
            for (a, b) in [(left, upper), (upper, right), (right, lower), (lower, left)] {
                draw_line(a.x, a.y, b.x, b.y, stroke, color);
            }
            draw_circle(center.x, center.y, radius * 0.28, color);
        }
        CombatIcon::Radar => {
            let origin = center + vec2(-radius * 0.58, radius * 0.58);
            draw_circle(origin.x, origin.y, radius * 0.18, color);
            for ring in [0.62, 1.0] {
                let segments = 7;
                for segment in 0..segments {
                    let a = -std::f32::consts::FRAC_PI_2
                        + std::f32::consts::FRAC_PI_2 * segment as f32 / segments as f32;
                    let b = -std::f32::consts::FRAC_PI_2
                        + std::f32::consts::FRAC_PI_2 * (segment + 1) as f32 / segments as f32;
                    draw_line(
                        origin.x + a.cos() * radius * ring,
                        origin.y + a.sin() * radius * ring,
                        origin.x + b.cos() * radius * ring,
                        origin.y + b.sin() * radius * ring,
                        stroke,
                        color,
                    );
                }
            }
        }
        CombatIcon::Repair => {
            draw_line(
                center.x - radius,
                center.y,
                center.x + radius,
                center.y,
                stroke * 1.25,
                color,
            );
            draw_line(
                center.x,
                center.y - radius,
                center.x,
                center.y + radius,
                stroke * 1.25,
                color,
            );
        }
    }
}

/// The user's UI scale preference — atomic f32 bits so the settings
/// screen can retune it live while every draw and hit-test path reads
/// it lock-free.
static USER_SCALE: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(f32::to_bits(1.0));

/// Installs the user scale factor (clamped to sane bounds; a config
/// promising 0x or 10x chrome must not brick the window).
pub fn set_user_scale(factor: f32) {
    USER_SCALE.store(
        f32::to_bits(factor.clamp(0.5, 3.0)),
        std::sync::atomic::Ordering::Relaxed,
    );
}

static REDUCED_MOTION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Installs the accessibility damp: decorative animation (alert
/// pulses, ping rings, muzzle flashes) holds still when set.
pub fn set_reduced_motion(on: bool) {
    REDUCED_MOTION.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Whether decorative animation is damped.
pub fn reduced_motion() -> bool {
    REDUCED_MOTION.load(std::sync::atomic::Ordering::Relaxed)
}

/// UI scale factor: chrome (text, bars, minimap) is authored in logical
/// pixels, and `screen_width()`/mouse coordinates are ALREADY logical —
/// macroquad's high-dpi backing store absorbs the retina multiple
/// underneath. Multiplying dpi in here double-sized every piece of
/// chrome for four releases (the audit's giant menus and viewport-
/// swallowing minimap, root-caused by a live probe: screen_w=1280 on a
/// 2560-pixel display). The user preference is the only factor.
pub fn ui_scale() -> f32 {
    let user = f32::from_bits(USER_SCALE.load(std::sync::atomic::Ordering::Relaxed));
    effective_ui_scale(user, viewport())
}

fn effective_ui_scale(user: f32, viewport: Vec2) -> f32 {
    // A narrow OR short window can't seat enlarged chrome. Width guards
    // horizontal packing; height keeps a 960x400 window from accepting
    // 150% cards that physically cannot fit even after the minimap yields.
    // The viewport is injected per frame, never queried from the window.
    let width_cap = (viewport.x / 640.0).max(1.0);
    let height_cap = (viewport.y / 400.0).max(1.0);
    user.min(width_cap).min(height_cap)
}

#[cfg(not(test))]
static VIEW_WIDTH: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
#[cfg(not(test))]
static VIEW_HEIGHT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

// Under cfg(test) the storage is thread-local: libtest runs each test
// on its own thread, so a test that injects a small window can't turn
// a concurrently running layout test red — nor, by panicking
// mid-test, leave the pollution behind for whoever runs next. Tests
// therefore need no restore call; every test thread starts at the
// 1280x800 headless default.
#[cfg(test)]
thread_local! {
    static VIEW: std::cell::Cell<(u32, u32)> = const { std::cell::Cell::new((0, 0)) };
}

/// The frame loop hands the window size in once per frame; chrome
/// scale math, menus, and session construction never query the window
/// themselves — which is what lets all of them run headless (the
/// default is the 1280x800 window).
#[cfg(not(test))]
pub fn set_viewport(w: f32, h: f32) {
    VIEW_WIDTH.store(w.to_bits(), std::sync::atomic::Ordering::Relaxed);
    VIEW_HEIGHT.store(h.to_bits(), std::sync::atomic::Ordering::Relaxed);
}

/// Test flavor of [`set_viewport`]: same contract, but the size is
/// this test thread's alone.
#[cfg(test)]
pub fn set_viewport(w: f32, h: f32) {
    VIEW.set((w.to_bits(), h.to_bits()));
}

/// The injected window size.
pub fn viewport() -> macroquad::prelude::Vec2 {
    macroquad::prelude::vec2(view_width(), view_height())
}

#[cfg(not(test))]
fn view_bits() -> (u32, u32) {
    (
        VIEW_WIDTH.load(std::sync::atomic::Ordering::Relaxed),
        VIEW_HEIGHT.load(std::sync::atomic::Ordering::Relaxed),
    )
}

#[cfg(test)]
fn view_bits() -> (u32, u32) {
    VIEW.with(std::cell::Cell::get)
}

fn view_width() -> f32 {
    match view_bits().0 {
        0 => 1280.0,
        bits => f32::from_bits(bits),
    }
}

fn view_height() -> f32 {
    match view_bits().1 {
        0 => 800.0,
        bits => f32::from_bits(bits),
    }
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
    // The debug overlay is deliberately omniscient; the spectator
    // stance (playback) skips the fog too but never the debug chrome.
    if game.overlay {
        draw_overlay(game, alpha);
    } else if !game.spectate {
        draw_fog(game);
    }
    // Own-order acknowledgments, rally flags, and radar blips sit above
    // the fog: they are the player's intent and intel, not world state.
    draw_pings(game);
    draw_range_rings(game, input);
    draw_blips(game);
    draw_rally_marker(game);
    draw_breadcrumbs(game, input);
    // Deferred claims are the player's own intent, like breadcrumbs —
    // a spectator has no chair whose promises deserve footprints.
    if !game.spectate {
        draw_pending_founds(game, sprites);
    }
    draw_placement_ghost(game, sprites, input);
    draw_drag_rect(game, input);
    draw_salvage_tooltip(game, input);
    draw_hud(game, sprites, input);
    draw_minimap(game);
    draw_result_overlay(game);
    draw_panel_tooltip(game, input);
}

const FOG_UNEXPLORED: Color = color_u8!(13, 13, 17, 255);
const FOG_EXPLORED: Color = color_u8!(22, 28, 44, 135);

fn visible_tiles(game: &Game) -> (TilePos, TilePos) {
    let (lo, hi) = game.camera.world_rect();
    let min = TilePos::new((lo.x.floor() as i32).max(0), (lo.y.floor() as i32).max(0));
    let max = TilePos::new(
        (hi.x.ceil() as i32).min(game.state.map().width()),
        (hi.y.ceil() as i32).min(game.state.map().height()),
    );
    (min, max)
}

/// Per-theme terrain grading: a subtle multiplier on ground-layer
/// sprites only. Units, chrome, the minimap, and the golden renderer
/// stay untinted — grading is atmosphere, never information.
pub fn theme_tint(theme: &str) -> Color {
    match theme {
        "rusted-yard" => Color::new(1.0, 0.95, 0.88, 1.0),
        "cold-circuitry" => Color::new(0.89, 0.95, 1.0, 1.0),
        "quarry-dust" => Color::new(1.0, 0.97, 0.90, 1.0),
        "basalt" => Color::new(0.92, 0.91, 1.0, 1.0),
        "slag" => Color::new(1.0, 0.92, 0.92, 1.0),
        "verdigris" => Color::new(0.90, 1.0, 0.94, 1.0),
        _ => WHITE,
    }
}

const GHOST_TINT: Color = color_u8!(150, 150, 165, 210);

fn draw_unit_pass(game: &Game, sprites: &Sprites, alpha: f32, domain: oxide_sim::stats::Domain) {
    let zoom = game.camera.zoom;
    let airborne = domain == oxide_sim::stats::Domain::Air;
    for unit in game.state.units() {
        if unit.kind.stats().domain != domain {
            continue;
        }
        if unit.player != game.human && !game.all_seeing() && !game.my_vision().visible(unit.tile())
        {
            continue;
        }
        let faction = game.state.player(unit.player).faction;
        let pos = game.draw_pos(unit.id, unit.pos, alpha);
        let mut screen = game.camera.to_screen(pos);
        let dest = zoom * 1.05;
        let current = vec2(unit.pos.x.to_num::<f32>(), unit.pos.y.to_num::<f32>());
        let moving = game
            .prev_pos
            .get(&unit.id.0)
            .is_some_and(|previous| (*previous - current).length_squared() > 1e-6);
        let pose = motion::unit_pose(
            game.fx_time(),
            unit.id.0,
            moving,
            airborne,
            reduced_motion(),
        );
        // A recent shot owns the heading: the chassis tracks its victim
        // for a beat, then movement facing resumes.
        let aim = game.aim_units.get(&unit.id.0).copied();
        let rotation = match aim {
            Some((angle, at)) if game.fx_time() - at < 1.2 => angle,
            _ => game.facing.get(&unit.id.0).copied().unwrap_or(0.0),
        };
        let forward = vec2(rotation.sin(), -rotation.cos());
        let right = vec2(rotation.cos(), rotation.sin());
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
        let mut body = screen + right * pose.lateral * dest + vec2(0.0, pose.lift * dest);
        if airborne && pose.thruster > 0.0 {
            // The steady reduced-motion flame still communicates that a
            // moving flyer is powered; only its flicker and bank freeze.
            let exhaust = body - forward * dest * 0.38;
            let half = right * dest * 0.055;
            draw_triangle(
                exhaust - half,
                exhaust + half,
                exhaust - forward * dest * (0.09 + 0.07 * pose.thruster),
                Color::new(0.95, 0.68, 0.28, 0.32 + 0.30 * pose.thruster),
            );
        }
        if game.selection.units.contains(&unit.id) {
            if unit.player == game.human {
                draw_circle_lines(
                    screen.x,
                    screen.y,
                    unit.kind.stats().radius.to_num::<f32>() * zoom + 4.0,
                    2.0,
                    BONE,
                );
            } else {
                // Inspected, not commanded: a fainter ring outside the
                // allegiance cue — "selected" and "mine" stay
                // different claims.
                draw_circle_lines(
                    screen.x,
                    screen.y,
                    unit.kind.stats().radius.to_num::<f32>() * zoom + 5.5,
                    1.5,
                    BONE_FAINT,
                );
            }
        }
        // Direct-fire chassis kick on top of the locomotion pose.
        if !reduced_motion()
            && let Some((angle, at)) = aim
        {
            let age = game.fx_time() - at;
            if age < 0.12 {
                let dir = vec2(angle.sin(), -angle.cos());
                body -= dir * zoom * 0.07 * (1.0 - age / 0.12);
            }
        }
        let body_size = vec2(dest * pose.width_scale, dest * pose.height_scale);
        // Locomotion and harvesting use whole authored frames, so mechanical
        // motion remains part of the machine rather than a detached overlay.
        let (source, accent) = if unit.kind == UnitKind::Harvester
            && matches!(unit.order, oxide_sim::Order::Harvest { node, .. }
                if unit.tile().chebyshev(node) <= 1)
        {
            let cycle = motion::loop_frame(game.fx_time(), unit.id.0, 4.0, 4, reduced_motion());
            let frame = [0usize, 1, 2, 1][cycle];
            (
                sprites.harvester_working(faction, frame),
                sprites.harvester_working_accent(frame),
            )
        } else if moving {
            let frame = motion::loop_frame(game.fx_time(), unit.id.0, 8.0, 3, reduced_motion());
            (
                sprites.unit_moving(unit.kind, faction, frame),
                sprites.unit_moving_accent(unit.kind, frame),
            )
        } else {
            (
                sprites.unit(unit.kind, faction),
                sprites.unit_accent(unit.kind),
            )
        };
        draw_texture_ex(
            sprites.texture(),
            body.x - body_size.x * 0.5,
            body.y - body_size.y * 0.5,
            WHITE,
            DrawTextureParams {
                dest_size: Some(body_size),
                source: Some(source),
                rotation,
                ..Default::default()
            },
        );
        // The allegiance accent rides the body draw exactly — same
        // pose, same frame — and draws UNCONDITIONALLY for non-own
        // machines: selection must never repaint a foe as a friend.
        if let Some(tint) = seat_identity_tint(game, unit.player) {
            draw_texture_ex(
                sprites.texture(),
                body.x - body_size.x * 0.5,
                body.y - body_size.y * 0.5,
                tint,
                DrawTextureParams {
                    dest_size: Some(body_size),
                    source: Some(accent),
                    rotation,
                    ..Default::default()
                },
            );
        }
        // The cargo eye: a fixed ring that FILLS with carrying/capacity
        // — load reads as area, not as a pulse (and needs no motion at
        // all). The scoop cycle above stays the "actually working"
        // tell; the eye only says how much is aboard.
        if unit.kind == UnitKind::Harvester
            && let Some(hstats) = unit.kind.stats().harvest
            && (unit.carrying > 0 || matches!(unit.order, oxide_sim::Order::Harvest { .. }))
        {
            let r = zoom * 0.11;
            let frac = (unit.carrying as f32 / hstats.capacity as f32).clamp(0.0, 1.0);
            draw_circle_lines(screen.x, screen.y, r, 1.0, SCRAP_COLOR);
            if frac > 0.0 {
                // Area-linear: half a load LOOKS half full.
                draw_circle(screen.x, screen.y, r * frac.sqrt(), SCRAP_COLOR);
            }
        }
        // Slow guns charge up through the same yellow circle: drawn
        // only while the shot is still coming back (an idle ready gun
        // wears nothing), for heavy cooldowns anywhere plus whatever
        // the player has selected. A spotter gun whose current victim
        // the team can't see hollows — a filling eye must not promise
        // a shot the fire gate is blocking.
        let stats = unit.kind.stats();
        if let Some(weapon) = stats.weapons.first() {
            let selected = game
                .selection
                .units
                .iter()
                .take(DECOR_CAP)
                .any(|i| *i == unit.id);
            let remaining = unit.cooldowns[0];
            if remaining > 0 && (weapon.cooldown_ticks >= CHARGE_EYE_COOLDOWN || selected) {
                let r = zoom * 0.11;
                let frac = 1.0 - remaining as f32 / weapon.cooldown_ticks as f32;
                let gated = unit.player == game.human
                    && weapon.range.to_num::<f32>() > stats.vision as f32
                    && match unit.order {
                        oxide_sim::Order::Attack { target, .. } => {
                            let tile = match target {
                                oxide_sim::Target::Unit(id) => {
                                    game.state.unit(id).map(|u| u.tile())
                                }
                                oxide_sim::Target::Building(id) => {
                                    game.state.building(id).map(|b| b.anchor)
                                }
                            };
                            tile.is_some_and(|t| !game.my_vision().visible(t))
                        }
                        _ => false,
                    };
                draw_circle_lines(screen.x, screen.y, r, 1.0, SCRAP_COLOR);
                if !gated && frac > 0.0 {
                    draw_circle(screen.x, screen.y, r * frac.sqrt(), SCRAP_COLOR);
                }
            }
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

/// Order-acknowledgment rings, drawn above the fog: they are the player's
/// own intent echoed back, not world intel to be hidden.
/// The range language: what a selected machine can shoot, see, and
/// detect — and the same rings under a placement ghost, because siting
/// a Flak Turret or Bastion IS the decision its rings describe. Weapon
/// reach draws in danger red, own vision in bone, the Array's radar
/// detection in patina teal, and Repair Bay healing in green; where a gun
/// outranges its own eyes
/// (Bombard, Bastion), the gap between red and bone is the spotter's
/// job, made visible.
/// How many selected units draw their rings and programs — a boxed
/// army of forty must not paint forty overlapping circles.
const DECOR_CAP: usize = 12;

/// Primary-weapon cooldown at which the charge eye draws unbidden
/// (lancer 60, bastion 90, bombard 100 — deliberately above the
/// Buzzard's 50, which flies in flocks and would wear twelve eyes).
const CHARGE_EYE_COOLDOWN: u32 = 55;

// --- Minimap ------------------------------------------------------------

/// The tutorial card's full rectangle — pure geometry shared by
/// drawing and input, which treats the card as chrome (clicks on an
/// instructional card must never reach the world).
pub fn tutorial_card_rect(t: &crate::tutorial::Tutorial) -> Rect {
    let s = ui_scale();
    let w = 460.0 * s;
    let x = (screen_width() - w) * 0.5;
    let lines = (crate::tutorial::STEPS
        .get(t.step)
        .map(|step| step.body.len())
        .unwrap_or(0)
        + usize::from(t.coach_active())) as f32;
    Rect::new(x, 36.0 * s, w, 34.0 * s + lines * 18.0 * s + 10.0 * s)
}

/// Where the tutorial card's dismiss box sits this frame.
pub fn tutorial_dismiss_rect() -> Rect {
    let s = ui_scale();
    let w = 460.0 * s;
    let x = (screen_width() - w) * 0.5;
    Rect::new(x + w - 26.0 * s, 40.0 * s, 22.0 * s, 22.0 * s)
}

/// The tutorial card: headline, lesson, live coach line, dismiss box,
/// progress. Drawn over the world, under nothing — school outranks
/// scenery.
pub fn draw_tutorial(t: &crate::tutorial::Tutorial, game: &crate::game::Game) {
    let Some(step) = crate::tutorial::STEPS.get(t.step) else {
        return;
    };
    let s = ui_scale();
    let rect = tutorial_card_rect(t);
    let (x, y, w, h) = (rect.x, rect.y, rect.w, rect.h);
    let line_h = 18.0 * s;
    draw_rectangle(x, y, w, h, SURFACE_CARD);
    draw_rectangle_lines(x, y, w, h, 1.5 * s, Color::new(0.85, 0.65, 0.35, 0.9));
    draw_text(
        format!(
            "TUTORIAL {}/{}  |  {}",
            t.step + 1,
            crate::tutorial::STEPS.len(),
            step.title
        ),
        x + 10.0 * s,
        y + 22.0 * s,
        18.0 * s,
        SCRAP_COLOR,
    );
    for (i, line) in step.body.iter().enumerate() {
        draw_text(
            line,
            x + 10.0 * s,
            y + 42.0 * s + i as f32 * line_h,
            15.0 * s,
            TEXT_BODY,
        );
    }
    if let Some(coach) = t.coach(game) {
        let color = match &coach {
            crate::tutorial::CoachLine::Status(_) => SCRAP_COLOR,
            crate::tutorial::CoachLine::Recovery(_) => DANGER,
        };
        draw_text(
            coach.text(),
            x + 10.0 * s,
            y + 42.0 * s + step.body.len() as f32 * line_h,
            15.0 * s,
            color,
        );
    }
    let d = tutorial_dismiss_rect();
    draw_rectangle_lines(d.x, d.y, d.w, d.h, 1.2 * s, TEXT_SECONDARY);
    draw_text("x", d.x + 7.0 * s, d.y + 16.0 * s, 16.0 * s, TEXT_SECONDARY);
}

#[cfg(test)]
mod tests {
    use macroquad::prelude::vec2;

    #[test]
    fn ui_scale_respects_both_small_window_axes() {
        assert_eq!(super::effective_ui_scale(1.5, vec2(640.0, 800.0)), 1.0);
        assert_eq!(super::effective_ui_scale(1.5, vec2(960.0, 400.0)), 1.0);
        assert_eq!(super::effective_ui_scale(1.5, vec2(960.0, 600.0)), 1.5);
        assert_eq!(super::effective_ui_scale(0.75, vec2(640.0, 400.0)), 0.75);
    }

    #[test]
    fn ground_and_air_weapon_marks_do_not_depend_on_one_hue() {
        let ground = super::combat_icon_color(crate::panel::CombatIcon::Weapon);
        let air = super::combat_icon_color(crate::panel::CombatIcon::AirWeapon);

        assert!(ground.r > ground.b, "ground range stays warm");
        assert!(air.b > air.r, "air range stays cool");
        assert_ne!(ground, air);
    }

    #[test]
    fn the_staleness_ramp_is_fresh_then_caps() {
        assert_eq!(super::staleness_fade(0.0), 0.0);
        assert!(super::staleness_fade(45.0) > 0.2);
        let capped = super::staleness_fade(600.0);
        assert!(
            (capped - 0.55).abs() < 1e-6,
            "old memories fade to a floor, never vanish"
        );
    }

    #[test]
    fn the_allegiance_cue_reads_teams_not_sprite_colors() {
        // Compass Grand pits the Ferrous human (seat 0) against a
        // Ferrous seat 4 — the exact ambiguity the cue exists for.
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../scenarios/compass-grand.json");
        let scenario = oxide_sim::Scenario::load(&path).expect("shipped map loads");
        let game =
            crate::game::Game::with_viewport(scenario, macroquad::prelude::vec2(1280.0, 800.0))
                .expect("compass grand builds");
        use super::AllegianceCue::*;
        let cue = |seat: u8| super::allegiance_cue(&game, oxide_sim::PlayerId(seat));
        assert_eq!(cue(0), Mine);
        assert_eq!(cue(1), Ally, "west Cupric teammate");
        assert_eq!(cue(2), Ally, "west Ferrous teammate");
        assert_eq!(
            cue(4),
            HostileTwin,
            "the Ferrous foe must not read as the player's own"
        );
        assert_eq!(cue(5), Hostile, "the Cupric foe reads by tint alone");
    }

    #[test]
    fn the_allegiance_tint_is_semantic_and_luminance_safe() {
        use super::AllegianceCue::*;
        super::set_colorblind(false);
        assert_eq!(super::allegiance_tint(Mine), None, "own machines stay pure");
        let ally = super::allegiance_tint(Ally).unwrap();
        let foe = super::allegiance_tint(Hostile).unwrap();
        assert_eq!(
            super::allegiance_tint(HostileTwin),
            Some(foe),
            "every hostile wears one hue; twins get no separate look"
        );
        assert!(ally.b > ally.r, "ally reads blue");
        assert!(foe.r > foe.b, "hostile reads crimson");

        super::set_colorblind(true);
        let cb_ally = super::allegiance_tint(Ally).unwrap();
        let cb_foe = super::allegiance_tint(Hostile).unwrap();
        let lum = |c: macroquad::prelude::Color| 0.299 * c.r + 0.587 * c.g + 0.114 * c.b;
        assert!(
            lum(cb_ally) - lum(cb_foe) > 0.3,
            "colorblind mode splits friend from foe by luminance, not hue"
        );
        super::set_colorblind(false);
    }

    #[test]
    fn every_team_seat_gets_a_stable_identity_inside_its_allegiance_family() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../scenarios/compass-grand.json");
        let scenario = oxide_sim::Scenario::load(&path).expect("shipped map loads");
        let game =
            crate::game::Game::with_viewport(scenario, macroquad::prelude::vec2(1280.0, 800.0))
                .expect("compass grand builds");
        super::set_colorblind(false);
        let key = |seat: u8| {
            let color = super::seat_identity_color(&game, oxide_sim::PlayerId(seat));
            (
                (color.r * 255.0).round() as u8,
                (color.g * 255.0).round() as u8,
                (color.b * 255.0).round() as u8,
            )
        };
        let allies = [1, 2, 3].map(key);
        let hostiles = [4, 5, 6, 7].map(key);
        for colors in [allies.as_slice(), hostiles.as_slice()] {
            for (index, color) in colors.iter().enumerate() {
                assert!(
                    !colors[..index].contains(color),
                    "each seat in one allegiance family needs a distinct accent"
                );
            }
        }
        assert!(
            allies.iter().all(|(r, _, b)| b > r),
            "allies stay in the cool family"
        );
        assert!(
            hostiles.iter().all(|(r, _, b)| r > b),
            "hostiles stay in the warm family"
        );
    }

    #[test]
    fn every_ffa_opponent_gets_a_distinct_hostile_identity() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../scenarios/compass-grand.json");
        let mut scenario = oxide_sim::Scenario::load(&path).expect("shipped map loads");
        for player in &mut scenario.players {
            player.team = None;
        }
        let game =
            crate::game::Game::with_viewport(scenario, macroquad::prelude::vec2(1280.0, 800.0))
                .expect("eight-seat FFA builds");
        super::set_colorblind(false);
        let colors: Vec<_> = (1..8)
            .map(|seat| {
                let color = super::seat_identity_color(&game, oxide_sim::PlayerId(seat));
                (
                    (color.r * 255.0).round() as u8,
                    (color.g * 255.0).round() as u8,
                    (color.b * 255.0).round() as u8,
                )
            })
            .collect();
        for (index, color) in colors.iter().enumerate() {
            assert!(
                !colors[..index].contains(color),
                "hostile seat {} aliases an earlier FFA opponent",
                index + 1
            );
        }
    }
}
