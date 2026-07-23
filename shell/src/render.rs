//! Drawing: map, entities, effects, HUD, debug overlay.
//!
//! Split by layer (0.10 file diet): submodules own the minimap and,
//! as the split continues, the panel, chrome, and world layers.
//!
//! Reads the sim, never writes it. Unit positions interpolate between the
//! previous and current tick so 20 sim ticks per second still looks like
//! 60fps motion.

use crate::assets::Sprites;
mod chrome;
mod minimap;
mod panel_draw;
use chrome::*;
pub use minimap::*;
use panel_draw::*;

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
    for id in game.selection.units.iter().take(DECOR_CAP) {
        let Some(unit) = game.state.unit(*id) else {
            continue;
        };
        // Only explored targets draw: the harvest brain can retarget to a
        // node the player has never seen, and a breadcrumb there would
        // leak it through the fog. Each verb speaks its own color: bone
        // walks, danger fights, scrap-gold harvests, patina builds and
        // welds — the program reads at a glance instead of as one gray
        // chain.
        let verb_color = |order: &oxide_sim::Order| match order {
            oxide_sim::Order::Move { .. } => BONE_FAINT,
            oxide_sim::Order::AttackMove { .. } | oxide_sim::Order::Attack { .. } => {
                Color::new(0.85, 0.32, 0.29, 0.55)
            }
            oxide_sim::Order::Harvest { .. } => Color::new(0.85, 0.64, 0.25, 0.55),
            oxide_sim::Order::Build { .. } | oxide_sim::Order::Repair { .. } => {
                Color::new(0.25, 0.58, 0.51, 0.55)
            }
            oxide_sim::Order::Idle => BONE_FAINT,
        };
        let goal_of = |order: &oxide_sim::Order| {
            let goal = match order {
                oxide_sim::Order::Move { goal } | oxide_sim::Order::AttackMove { goal } => *goal,
                oxide_sim::Order::Harvest { node } => *node,
                oxide_sim::Order::Build { site } => game.state.building(*site)?.anchor,
                oxide_sim::Order::Repair { building } => game.state.building(*building)?.anchor,
                oxide_sim::Order::Attack { target, .. } => {
                    // A chase target draws only while its ground is
                    // seen — the victim may have slipped back into fog.
                    let tile = match target {
                        oxide_sim::Target::Unit(uid) => game.state.unit(*uid)?.tile(),
                        oxide_sim::Target::Building(bid) => game.state.building(*bid)?.anchor,
                    };
                    if game.all_seeing() || game.my_vision().visible(tile) {
                        return Some((tile, verb_color(order)));
                    }
                    return None;
                }
                oxide_sim::Order::Idle => return None,
            };
            (game.all_seeing() || game.my_vision().explored(goal))
                .then_some((goal, verb_color(order)))
        };
        let mut points: Vec<(Vec2, Color)> = Vec::new();
        if let Some((g, c)) = goal_of(&unit.order) {
            points.push((
                game.camera
                    .to_screen(vec2(g.x as f32 + 0.5, g.y as f32 + 0.5)),
                c,
            ));
        }
        for order in &unit.queue {
            if let Some((g, c)) = goal_of(order) {
                points.push((
                    game.camera
                        .to_screen(vec2(g.x as f32 + 0.5, g.y as f32 + 0.5)),
                    c,
                ));
            }
        }
        if points.is_empty() {
            continue;
        }
        let start = game
            .camera
            .to_screen(vec2(unit.pos.x.to_num::<f32>(), unit.pos.y.to_num::<f32>()));
        let s = ui_scale();
        let mut prev = start;
        for (i, (p, color)) in points.iter().enumerate() {
            draw_line(prev.x, prev.y, p.x, p.y, 1.0, *color);
            dot(*p, *color);
            // Numbered waypoints once a program has legs.
            if points.len() > 1 {
                draw_text(
                    format!("{}", i + 1),
                    p.x + 6.0 * s,
                    p.y - 4.0 * s,
                    14.0 * s,
                    *color,
                );
            }
            prev = *p;
        }
        // A patrol is a circuit: close it.
        if unit.looping && points.len() > 1 {
            let (first, color) = points[0];
            draw_line(prev.x, prev.y, first.x, first.y, 1.0, color);
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

fn draw_tiles(game: &Game, sprites: &Sprites) {
    let zoom = game.camera.zoom;
    let size = zoom.ceil() + 1.0; // slight overlap kills seam hairlines
    let tint = theme_tint(
        game.scenario
            .meta
            .as_ref()
            .map(|m| m.theme.as_str())
            .unwrap_or(""),
    );
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
                tint,
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
                    tint,
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
                            tint,
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
            let scrap = if game.all_seeing() || game.my_vision().visible(pos) {
                tile.scrap
            } else {
                game.my_vision().remembered_scrap(pos)
            };
            // Wrecks follow the same sight rule; a live node or rock
            // outranks the junk visually.
            let wreck = if game.all_seeing() || game.my_vision().visible(pos) {
                tile.wreck
            } else {
                game.my_vision().remembered_wreck(pos)
            };
            let (overlay, flip) = match (tile.terrain, scrap) {
                (oxide_sim::map::Terrain::Rock, _) => (Some(sprites.rock(h % 4)), h % 7 < 3),
                (oxide_sim::map::Terrain::Peak, _) => {
                    // Connectivity picks the art: interior wall tiles
                    // read as solid rock, the skyline row carries the
                    // crests, and connected edges share a fixed profile
                    // so ridges join without seams. Never flipped —
                    // a flip would break those joins.
                    let peaky = |dx: i32, dy: i32| {
                        let pos = TilePos::new(x + dx, y + dy);
                        // An unexplored neighbor is unknown, not absent:
                        // reading its live terrain would let a known
                        // peak's edge art disclose whether the ridge
                        // continues under fog.
                        (game.all_seeing() || game.my_vision().explored(pos))
                            && game
                                .state
                                .map()
                                .tile(pos)
                                .is_some_and(|t| t.terrain == oxide_sim::map::Terrain::Peak)
                    };
                    let source = if peaky(0, -1) {
                        sprites.peak_body(h % 2)
                    } else if !peaky(-1, 0) && !peaky(1, 0) && !peaky(0, 1) {
                        sprites.peak_lone(h % 2)
                    } else {
                        sprites.peak_sky(peaky(-1, 0), peaky(1, 0), h % 2)
                    };
                    (Some(source), false)
                }
                (_, 0) if wreck > 0 => (Some(sprites.wreck_pile()), h % 5 < 2),
                (_, 0) => (None, false),
                (_, s) => (Some(sprites.scrap(s, SCRAP_NODE_AMOUNT)), false),
            };
            if let Some(source) = overlay {
                draw_texture_ex(
                    sprites.texture(),
                    screen.x.floor(),
                    screen.y.floor(),
                    tint,
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
    if !game.all_seeing() {
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
            if ghost.kind == oxide_sim::BuildingKind::Turret {
                // The base ships bare; the remembered gun points up.
                draw_texture_ex(
                    sprites.texture(),
                    screen.x,
                    screen.y,
                    tint,
                    DrawTextureParams {
                        dest_size: Some(vec2(w as f32 * zoom, h as f32 * zoom)),
                        source: Some(sprites.turret_barrel(faction)),
                        ..Default::default()
                    },
                );
            }
        }
    }
    for building in game.state.buildings() {
        if building.player != game.human
            && !game.all_seeing()
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
        // Sites render translucent and solidify as they rise — the
        // alpha IS the construction stage.
        let tint = if building.built {
            WHITE
        } else {
            let ticks = building
                .kind
                .stats()
                .construction
                .map(|c| c.build_ticks)
                .unwrap_or(1);
            let frac = (building.progress as f32 / ticks as f32).clamp(0.0, 1.0);
            Color::new(1.0, 1.0, 1.0, 0.35 + 0.45 * frac)
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
        if building.built {
            let center = vec2(screen.x + dest.x * 0.5, screen.y + dest.y * 0.5);
            match building.kind {
                // Guns wear their aim in their own idiom: the Turret's
                // gun is a separate sprite that tracks (with recoil);
                // the flak battery flashes its skyward quad; the
                // Bastion's mortar throat glows on launch. Painting one
                // generic barrel over all three doubled the turret's
                // art and contradicted the other two entirely.
                oxide_sim::BuildingKind::Turret => {
                    let (angle, age) = game
                        .aim_buildings
                        .get(&building.id.0)
                        .map(|(a, at)| (*a, game.fx_time() - at))
                        .unwrap_or((0.0, f32::MAX));
                    let dir = vec2(angle.sin(), -angle.cos());
                    let kick = if !reduced_motion() && age < 0.12 {
                        -dir * dest.x * 0.05 * (1.0 - age / 0.12)
                    } else {
                        vec2(0.0, 0.0)
                    };
                    let size = dest.x * 1.0;
                    let at = center + kick - vec2(size, size) * 0.5;
                    draw_texture_ex(
                        sprites.texture(),
                        at.x,
                        at.y,
                        WHITE,
                        DrawTextureParams {
                            dest_size: Some(vec2(size, size)),
                            source: Some(sprites.turret_barrel(faction)),
                            rotation: angle,
                            ..Default::default()
                        },
                    );
                }
                oxide_sim::BuildingKind::FlakTurret => {
                    if let Some((_, at)) = game.aim_buildings.get(&building.id.0) {
                        let age = game.fx_time() - at;
                        if age < 0.18 && !reduced_motion() {
                            let a = 1.0 - age / 0.18;
                            for (ox, oy) in [(0.39, 0.39), (0.61, 0.39), (0.39, 0.61), (0.61, 0.61)]
                            {
                                draw_circle(
                                    screen.x + dest.x * ox,
                                    screen.y + dest.y * oy,
                                    dest.x * 0.05,
                                    Color::new(0.95, 0.9, 0.7, 0.8 * a),
                                );
                            }
                        }
                    }
                }
                oxide_sim::BuildingKind::Bastion => {
                    if let Some((_, at)) = game.aim_buildings.get(&building.id.0) {
                        let age = game.fx_time() - at;
                        if age < 0.3 && !reduced_motion() {
                            let a = 1.0 - age / 0.3;
                            draw_circle(
                                center.x,
                                center.y,
                                dest.x * (0.10 + 0.05 * a),
                                Color::new(0.98, 0.8, 0.5, 0.7 * a),
                            );
                        }
                    }
                }
                // The radar sweeps its ring — damped to a steady mast.
                oxide_sim::BuildingKind::Array => {
                    if !reduced_motion() {
                        let sweep = (get_time() * 1.1) as f32 % (2.0 * std::f32::consts::PI);
                        let reach = zoom * 4.0;
                        let tip = center + vec2(sweep.cos(), sweep.sin()) * reach;
                        draw_line(
                            center.x,
                            center.y,
                            tip.x,
                            tip.y,
                            1.5,
                            Color::new(0.55, 0.87, 0.78, 0.20),
                        );
                    }
                }
                // The reclaimer breathes its trickle.
                oxide_sim::BuildingKind::Reclaimer => {
                    let pulse = if reduced_motion() {
                        0.5
                    } else {
                        ((get_time() * 1.7 + f64::from(building.id.0)).sin() * 0.5 + 0.5) as f32
                    };
                    draw_circle(
                        center.x,
                        center.y,
                        dest.x * 0.18 * (1.0 + 0.1 * pulse),
                        Color::new(0.75, 0.68, 0.4, 0.08 + 0.08 * pulse),
                    );
                }
                // The fabricator's work light blinks.
                oxide_sim::BuildingKind::Fabricator => {
                    let on = reduced_motion()
                        || ((get_time() * 1.4 + f64::from(building.id.0)).fract() < 0.5);
                    if on {
                        draw_circle(
                            screen.x + dest.x * 0.82,
                            screen.y + dest.y * 0.18,
                            2.5 * ui_scale(),
                            SCRAP_COLOR,
                        );
                    }
                }
                _ => {}
            }
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
        draw_texture_ex(
            sprites.texture(),
            body.x - dest * 0.5,
            body.y - dest * 0.5,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(dest, dest)),
                source: Some(sprites.unit(unit.kind, faction)),
                rotation,
                ..Default::default()
            },
        );
        if unit.kind == UnitKind::Harvester && unit.carrying > 0 {
            let bob = if reduced_motion() {
                0.0
            } else {
                ((get_time() * 7.0 + f64::from(unit.id.0)).sin() * 0.015) as f32
            };
            draw_circle(screen.x, screen.y, zoom * (0.09 + bob), SCRAP_COLOR);
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
    // Real shells render from sim state, aged by sim ticks: pause holds
    // them mid-air, speed changes track, and a replay loaded mid-flight
    // restores them — no wall-clock effect can drift from the rules.
    let shell_speed = oxide_sim::stats::SHELL_SPEED.to_num::<f32>();
    let now = game.state.current_tick() as f32 + game.tick_fraction();
    for shell in game.state.shells() {
        let from = vec2(
            shell.launch.x.to_num::<f32>(),
            shell.launch.y.to_num::<f32>(),
        );
        let to = vec2(
            shell.impact.x.to_num::<f32>(),
            shell.impact.y.to_num::<f32>(),
        );
        // Fog rule: own and allied shells draw whole; a hostile arc
        // draws only segments crossing ground the player can see.
        // Anchoring a trail at a fogged muzzle would pinpoint exactly
        // the hidden artillery the spotter-weapon design protects —
        // the sim's incoming-shell sense exposes the impact tile,
        // never the launch, and the renderer must match it.
        let mine = !game.state.hostile(game.human, shell.player);
        let flat_seen = |k: f32| sees(from.lerp(to, k));
        if !game.all_seeing() && !mine && !(0..=10).any(|i| flat_seen(i as f32 / 10.0)) {
            continue;
        }
        // Reconstruct flight length the way the launch computed it, so
        // the dot lands exactly when the sim resolves the hit.
        let total = (from.distance(to) / shell_speed).ceil().max(1.0);
        let elapsed = total - (shell.arrival as f32 - now);
        let t = (elapsed / total).clamp(0.0, 1.0);
        let a = game.camera.to_screen(from);
        let b = game.camera.to_screen(to);
        let dist = (b - a).length();
        let lift = (dist * 0.22).min(game.camera.zoom * 3.0);
        let at = |t: f32| {
            let flat = a.lerp(b, t);
            vec2(flat.x, flat.y - lift * 4.0 * t * (1.0 - t))
        };
        let mut prev = at(0.0);
        let steps = 10;
        for i in 1..=((t * steps as f32) as usize).max(1) {
            let p = at(i as f32 / steps as f32);
            let visible = game.all_seeing()
                || mine
                || (flat_seen((i - 1) as f32 / steps as f32) && flat_seen(i as f32 / steps as f32));
            if visible {
                let fade = 0.35 * (1.0 - t);
                draw_line(
                    prev.x,
                    prev.y,
                    p.x,
                    p.y,
                    1.5,
                    Color::new(0.95, 0.75, 0.5, fade),
                );
            }
            prev = p;
        }
        if !(game.all_seeing() || mine || flat_seen(t)) {
            continue;
        }
        let dot = at(t);
        draw_circle(
            dot.x,
            dot.y,
            3.0,
            Color::new(0.98, 0.93, 0.8, 1.0 - t * 0.5),
        );
    }
    for fx in &game.fx {
        // A beam needs BOTH endpoints in sight: a half-fogged laser would
        // pinpoint an unseen combatant at its far end.
        let in_sight = match fx.kind {
            EffectKind::Bolt { from, to, .. } => sees(from) && sees(to),
            EffectKind::Puff { at } => sees(at),
            EffectKind::Falling { at, .. } => sees(at),
            EffectKind::Burst { at, .. } => sees(at),
            // Own-order acknowledgments always show; fogged targets are
            // already impossible to order onto.
            EffectKind::Ping { .. } => true,
        };
        if !game.all_seeing() && !in_sight {
            continue;
        }
        match fx.kind {
            EffectKind::Bolt { style, from, to } => {
                use crate::game::BoltStyle;
                let a = game.camera.to_screen(from);
                let b = game.camera.to_screen(to);
                let fade = (1.0 - fx.age / style.life()).clamp(0.0, 1.0);
                let (w, glow, core) = match style {
                    BoltStyle::Tracer => (
                        1.0,
                        Color::new(0.95, 0.75, 0.5, 0.22 * fade),
                        Color::new(0.98, 0.93, 0.8, fade),
                    ),
                    BoltStyle::Rail => (
                        2.0,
                        Color::new(0.75, 0.85, 1.0, 0.28 * fade),
                        Color::new(0.92, 0.96, 1.0, fade),
                    ),
                    BoltStyle::Flak => (
                        0.8,
                        Color::new(0.85, 0.85, 0.75, 0.15 * fade),
                        Color::new(0.9, 0.9, 0.82, 0.7 * fade),
                    ),
                    BoltStyle::AirStrike => (
                        1.4,
                        Color::new(0.55, 0.9, 0.8, 0.25 * fade),
                        Color::new(0.8, 1.0, 0.94, fade),
                    ),
                };
                draw_line(a.x, a.y, b.x, b.y, 7.0 * w * fade.max(0.3), glow);
                draw_line(a.x, a.y, b.x, b.y, 2.5 * w * fade.max(0.2), core);
                // Flak detonates in the air around its target: three
                // pseudo-random puffs blooming outward as the bolt ages.
                if style == BoltStyle::Flak {
                    let h = (to.x * 31.7 + to.y * 17.3).abs();
                    for i in 0..3 {
                        let angle = h + i as f32 * 2.1;
                        let reach = (fx.age / style.life()) * game.camera.zoom * 0.6;
                        let puff = b + vec2(angle.cos(), angle.sin()) * reach;
                        draw_circle(
                            puff.x,
                            puff.y,
                            game.camera.zoom * 0.12 * (1.0 - fx.age / style.life() * 0.5),
                            Color::new(0.88, 0.88, 0.8, 0.5 * fade),
                        );
                    }
                }
                if fx.age < 0.07 && !reduced_motion() {
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
            EffectKind::Falling { at, unit, faction } => {
                // Gravity takes the wreck: drop accelerates, the hull
                // spins and shrinks, and the ground swallows it.
                let t = (fx.age / 0.7).clamp(0.0, 1.0);
                let world = vec2(at.x, at.y + t * t * 1.4);
                let screen = game.camera.to_screen(world);
                let size = game.camera.zoom * 1.05 * (1.0 - t * 0.55);
                draw_texture_ex(
                    sprites.texture(),
                    screen.x - size * 0.5,
                    screen.y - size * 0.5,
                    Color::new(1.0, 1.0, 1.0, 1.0 - t * 0.8),
                    DrawTextureParams {
                        dest_size: Some(vec2(size, size)),
                        source: Some(sprites.unit(unit, faction)),
                        rotation: t * 5.2,
                        ..Default::default()
                    },
                );
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

fn draw_range_rings(game: &Game, input: &InputState) {
    let s = ui_scale();
    let ring = |world: Vec2, radius: f32, color: Color| {
        if radius <= 0.0 {
            return;
        }
        let center = game.camera.to_screen(world);
        draw_circle_lines(
            center.x,
            center.y,
            radius * game.camera.zoom,
            1.5 * s,
            color,
        );
    };
    let weapon_color = Color::new(0.85, 0.32, 0.29, 0.55);
    let sidearm_color = Color::new(0.85, 0.32, 0.29, 0.30);
    let vision_color = Color::new(0.91, 0.89, 0.85, 0.25);
    let radar_color = Color::new(0.25, 0.58, 0.51, 0.45);

    let unit_rings = |world: Vec2, stats: &oxide_sim::stats::UnitStats| {
        for (i, weapon) in stats.weapons.iter().enumerate() {
            let color = if i == 0 { weapon_color } else { sidearm_color };
            ring(world, weapon.range.to_num::<f32>(), color);
        }
        // Guns past their own eyes need a spotter: show the gap.
        if stats
            .weapons
            .iter()
            .any(|w| w.range.to_num::<f32>() > stats.vision as f32)
        {
            ring(world, stats.vision as f32, vision_color);
        }
    };
    let building_rings = |world: Vec2, kind: oxide_sim::BuildingKind| {
        let stats = kind.stats();
        if let Some(weapon) = stats.weapons.first() {
            ring(world, weapon.range.to_num::<f32>(), weapon_color);
            if weapon.range.to_num::<f32>() > stats.vision as f32 {
                ring(world, stats.vision as f32, vision_color);
            }
        }
        if kind == oxide_sim::BuildingKind::Array {
            ring(world, stats.vision as f32, vision_color);
            ring(
                world,
                oxide_sim::stats::RADAR_DETECT_RADIUS as f32,
                radar_color,
            );
        }
    };

    for id in game.selection.units.iter().take(DECOR_CAP) {
        if let Some(unit) = game.state.unit(*id) {
            let world = vec2(unit.pos.x.to_num::<f32>(), unit.pos.y.to_num::<f32>());
            unit_rings(world, unit.kind.stats());
        }
    }
    if let Some(id) = game.selection.building
        && let Some(building) = game.state.building(id)
    {
        let center = building.center();
        building_rings(
            vec2(center.x.to_num::<f32>(), center.y.to_num::<f32>()),
            building.kind,
        );
    }
    // The armed placement ghost carries its rings to the cursor.
    if let Some(kind) = input.placing {
        let world = game.camera.to_world(input.mouse);
        let size = kind.stats().size;
        let anchor = vec2(world.x.floor(), world.y.floor());
        let center = anchor + vec2(size.0 as f32 * 0.5, size.1 as f32 * 0.5);
        building_rings(center, kind);
    }
}

fn draw_pings(game: &Game) {
    for fx in &game.fx {
        let EffectKind::Ping { at, kind } = fx.kind else {
            continue;
        };
        let center = game.camera.to_screen(at);
        let progress = (fx.age / 0.5).clamp(0.0, 1.0);
        // Damped: a still ring instead of a collapsing one — the verb
        // color still says what was ordered.
        let radius = if reduced_motion() {
            game.camera.zoom * 0.4
        } else {
            game.camera.zoom * (0.65 * (1.0 - progress) + 0.12)
        };
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
    // A selected producer draws the line to its rally, not just the
    // flag — where fresh machines will walk should read at a glance.
    if let Some(id) = game.selection.building
        && let Some(building) = game.state.building(id)
        && let Some(rally) = building.rally
    {
        let a = game.camera.to_screen(vec2(
            building.anchor.x as f32 + building.kind.stats().size.0 as f32 * 0.5,
            building.anchor.y as f32 + building.kind.stats().size.1 as f32 * 0.5,
        ));
        let b = game
            .camera
            .to_screen(vec2(rally.x as f32 + 0.5, rally.y as f32 + 0.5));
        draw_line(a.x, a.y, b.x, b.y, 1.5, Color::new(0.91, 0.89, 0.85, 0.35));
    }
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
        if origin.distance(now) > crate::input::drag_threshold(ui_scale()) {
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
