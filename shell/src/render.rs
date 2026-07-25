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

/// How faded a memory draws after `age` seconds unseen: 0 fresh,
/// climbing to a 0.55 fade over ninety seconds. Memories never vanish
/// — the player recorded them honestly — they just stop pretending to
/// be news.
pub fn staleness_fade(age: f32) -> f32 {
    (age / 90.0).clamp(0.0, 1.0) * 0.55
}

mod chrome;
mod entities;
mod minimap;
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
use oxide_sim::stats::SCRAP_NODE_AMOUNT;
use oxide_sim::{GameResult, UnitKind};

const OUTSIDE: Color = color_u8!(20, 20, 25, 255);
const BONE: Color = color_u8!(232, 228, 216, 255);
const BONE_FAINT: Color = color_u8!(232, 228, 216, 90);
const SCRAP_COLOR: Color = color_u8!(217, 164, 65, 255);
const HP_BACK: Color = color_u8!(20, 20, 24, 220);
const DANGER: Color = color_u8!(217, 82, 74, 255);
const PANEL: Color = color_u8!(20, 20, 24, 200);

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
    // A narrow window can't seat 150% chrome: panel packing would run
    // under the minimap and its click rects would shadow camera clicks.
    // Cap by width so 640px tops out at 1x, 960px at 1.5x, and roomy
    // windows keep whatever the user asked for. The width is injected
    // per frame, never queried — headless tests get the default window.
    let cap = (view_width() / 640.0).max(1.0);
    user.min(cap)
}

static VIEW_WIDTH: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static VIEW_HEIGHT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The frame loop hands the window size in once per frame; chrome
/// scale math, menus, and session construction never query the window
/// themselves — which is what lets all of them run headless (the
/// default is the 1280x800 window).
pub fn set_viewport(w: f32, h: f32) {
    VIEW_WIDTH.store(w.to_bits(), std::sync::atomic::Ordering::Relaxed);
    VIEW_HEIGHT.store(h.to_bits(), std::sync::atomic::Ordering::Relaxed);
}

/// The injected window size.
pub fn viewport() -> macroquad::prelude::Vec2 {
    macroquad::prelude::vec2(view_width(), view_height())
}

fn view_width() -> f32 {
    match VIEW_WIDTH.load(std::sync::atomic::Ordering::Relaxed) {
        0 => 1280.0,
        bits => f32::from_bits(bits),
    }
}

fn view_height() -> f32 {
    match VIEW_HEIGHT.load(std::sync::atomic::Ordering::Relaxed) {
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
        } else {
            match allegiance_cue(game, unit.player) {
                // Teammates wear a soft whitened ring — same language
                // as the minimap's ally lift.
                AllegianceCue::Ally => draw_circle_lines(
                    screen.x,
                    screen.y,
                    unit.kind.stats().radius.to_num::<f32>() * zoom + 3.0,
                    1.5,
                    Color::new(0.95, 0.95, 0.9, 0.55),
                ),
                // The mirror case rings dark: luminance says foe where
                // tint cannot, whatever the viewer's color vision.
                AllegianceCue::HostileTwin => draw_circle_lines(
                    screen.x,
                    screen.y,
                    unit.kind.stats().radius.to_num::<f32>() * zoom + 3.0,
                    1.5,
                    Color::new(0.05, 0.05, 0.07, 0.7),
                ),
                AllegianceCue::Mine | AllegianceCue::Hostile => {}
            }
        }
        // A recent shot owns the heading: the mount tracks its victim
        // for a beat, with a recoil nudge fading over the first tenth
        // of a second, then movement facing resumes.
        let aim = game.aim_units.get(&unit.id.0).copied();
        let rotation = match aim {
            Some((angle, at)) if game.fx_time() - at < 1.2 => angle,
            _ => game.facing.get(&unit.id.0).copied().unwrap_or(0.0),
        };
        let mut body = screen;
        if !reduced_motion()
            && let Some((angle, at)) = aim
        {
            let age = game.fx_time() - at;
            if age < 0.12 {
                let dir = vec2(angle.sin(), -angle.cos());
                body -= dir * zoom * 0.07 * (1.0 - age / 0.12);
            }
        }
        // A working harvester runs its scoop cycle — dig frames while it
        // stands at its source, the travel pose everywhere else. Under
        // reduced motion the cycle freezes on the travel pose.
        let source = if unit.kind == UnitKind::Harvester
            && !reduced_motion()
            && matches!(unit.order, oxide_sim::Order::Harvest { node }
                if unit.tile().chebyshev(node) <= 1)
        {
            let frame = [0usize, 1, 2, 1][((game.fx_time() * 4.0) as usize) % 4];
            sprites.harvester_working(faction, frame)
        } else {
            sprites.unit(unit.kind, faction)
        };
        draw_texture_ex(
            sprites.texture(),
            body.x - dest * 0.5,
            body.y - dest * 0.5,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(dest, dest)),
                source: Some(source),
                rotation,
                ..Default::default()
            },
        );
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
/// detection in patina teal; where a gun outranges its own eyes
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
    let lines = crate::tutorial::STEPS
        .get(t.step)
        .map(|step| step.body.len())
        .unwrap_or(0) as f32;
    Rect::new(x, 36.0 * s, w, 34.0 * s + lines * 18.0 * s + 10.0 * s)
}

/// Where the tutorial card's dismiss box sits this frame.
pub fn tutorial_dismiss_rect() -> Rect {
    let s = ui_scale();
    let w = 460.0 * s;
    let x = (screen_width() - w) * 0.5;
    Rect::new(x + w - 26.0 * s, 40.0 * s, 22.0 * s, 22.0 * s)
}

/// The tutorial card: headline, lesson, dismiss box, progress. Drawn
/// over the world, under nothing — school outranks scenery.
pub fn draw_tutorial(t: &crate::tutorial::Tutorial) {
    let Some(step) = crate::tutorial::STEPS.get(t.step) else {
        return;
    };
    let s = ui_scale();
    let w = 460.0 * s;
    let x = (screen_width() - w) * 0.5;
    let y = 36.0 * s;
    let line_h = 18.0 * s;
    let h = 34.0 * s + step.body.len() as f32 * line_h + 10.0 * s;
    draw_rectangle(x, y, w, h, Color::from_rgba(14, 14, 18, 235));
    draw_rectangle_lines(x, y, w, h, 1.5 * s, Color::new(0.85, 0.65, 0.35, 0.9));
    draw_text(
        format!(
            "TUTORIAL {}/{}  ·  {}",
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
            BONE_FAINT,
        );
    }
    let d = tutorial_dismiss_rect();
    draw_rectangle_lines(d.x, d.y, d.w, d.h, 1.2 * s, BONE_FAINT);
    draw_text("x", d.x + 7.0 * s, d.y + 16.0 * s, 16.0 * s, BONE_FAINT);
}

#[cfg(test)]
mod tests {
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
}
