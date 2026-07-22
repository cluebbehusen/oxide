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
    // windows keep whatever the user asked for.
    let cap = (screen_width() / 640.0).max(1.0);
    user.min(cap)
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
    draw_range_rings(game, input);
    draw_blips(game);
    draw_rally_marker(game);
    draw_breadcrumbs(game, input);
    draw_placement_ghost(game, sprites, input);
    draw_drag_rect(game, input);
    draw_salvage_tooltip(game, input);
    draw_hud(game, sprites, input);
    draw_minimap(game);
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
    for id in &game.selection.units {
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
                    if game.overlay || game.my_vision().visible(tile) {
                        return Some((tile, verb_color(order)));
                    }
                    return None;
                }
                oxide_sim::Order::Idle => return None,
            };
            (game.overlay || game.my_vision().explored(goal)).then_some((goal, verb_color(order)))
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
                (oxide_sim::map::Terrain::Peak, _) => (Some(sprites.peak(h % 2)), h % 7 < 3),
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
                // Guns wear their aim: a barrel tracking the last victim
                // (default up), with recoil in the first tenth-second.
                oxide_sim::BuildingKind::Turret
                | oxide_sim::BuildingKind::FlakTurret
                | oxide_sim::BuildingKind::Bastion => {
                    let (angle, age) = game
                        .aim_buildings
                        .get(&building.id.0)
                        .map(|(a, at)| (*a, game.fx_time() - at))
                        .unwrap_or((0.0, f32::MAX));
                    let dir = vec2(angle.sin(), -angle.cos());
                    let heavy = building.kind == oxide_sim::BuildingKind::Bastion;
                    let len = dest.x * if heavy { 0.5 } else { 0.42 };
                    let width = dest.x * if heavy { 0.13 } else { 0.09 };
                    let kick = if !reduced_motion() && age < 0.12 {
                        -dir * dest.x * 0.06 * (1.0 - age / 0.12)
                    } else {
                        vec2(0.0, 0.0)
                    };
                    let base = center + kick;
                    let tip = base + dir * len;
                    draw_line(
                        base.x,
                        base.y,
                        tip.x,
                        tip.y,
                        width,
                        Color::new(0.15, 0.15, 0.18, 1.0),
                    );
                    draw_line(base.x, base.y, tip.x, tip.y, width * 0.45, BONE_FAINT);
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
        if !game.overlay && !(sees(from) || sees(to)) {
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
            let fade = 0.35 * (1.0 - t);
            draw_line(
                prev.x,
                prev.y,
                p.x,
                p.y,
                1.5,
                Color::new(0.95, 0.75, 0.5, fade),
            );
            prev = p;
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
        if !game.overlay && !in_sight {
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

    for id in &game.selection.units {
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

/// Hovered salvage says what it holds: live amounts on visible ground,
/// remembered amounts under the dim — the same memory rule as every
/// renderer, so the tooltip can't leak what fog took back.
fn draw_salvage_tooltip(game: &Game, input: &InputState) {
    if game.layout.get().chrome_owns(input.mouse) {
        return;
    }
    let world = game.camera.to_world(input.mouse);
    let tile = TilePos::new(world.x.floor() as i32, world.y.floor() as i32);
    let vision = game.my_vision();
    if !vision.explored(tile) && !game.overlay {
        return;
    }
    let (scrap, wreck) = if vision.visible(tile) || game.overlay {
        (
            game.state.map().scrap_at(tile),
            game.state.map().wreck_at(tile),
        )
    } else {
        (vision.remembered_scrap(tile), vision.remembered_wreck(tile))
    };
    let text = match (scrap > 0, wreck > 0) {
        (true, _) => format!("scrap {scrap}"),
        (_, true) => format!("wreck {wreck}"),
        _ => return,
    };
    let s = ui_scale();
    let dims = measure_text(&text, None, (16.0 * s) as u16, 1.0);
    let (x, y) = (input.mouse.x + 14.0 * s, input.mouse.y - 10.0 * s);
    draw_rectangle(
        x - 4.0 * s,
        y - 14.0 * s,
        dims.width + 8.0 * s,
        20.0 * s,
        PANEL,
    );
    draw_text(&text, x, y, 16.0 * s, SCRAP_COLOR);
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

fn draw_hud(game: &Game, sprites: &Sprites, input: &InputState) {
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
    // Idle harvesters are money on the ground; the badge nags in danger
    // red and clicking it (or N) cycles through them.
    let idle = crate::input::idle_harvesters(game).len();
    let idle_badge = if idle > 0 {
        let label = format!("IDLE {idle}");
        let dims = measure_text(&label, None, (22.0 * s) as u16, 1.0);
        let x = 360.0 * s;
        draw_text(&label, x, 22.0 * s, 22.0 * s, DANGER);
        Rect::new(x - 4.0 * s, 4.0 * s, dims.width + 8.0 * s, 26.0 * s)
    } else {
        Rect::new(0.0, 0.0, 0.0, 0.0)
    };
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

    let panel = crate::panel::build(game, &input.bindings);
    let panel_shown = panel.is_some();
    let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut cards = [(zero, crate::panel::CardAction::None); 16];
    let mut card_count = 0;
    let mut queue_slots = [(zero, crate::panel::CardAction::None); 8];
    let mut queue_count = 0;
    let mut panel_top = f32::INFINITY;
    if let Some(panel) = &panel {
        let (c, cc, q, qc, top) = draw_panel(game, sprites, input, panel);
        cards = c;
        card_count = cc;
        queue_slots = q;
        queue_count = qc;
        panel_top = top;
    }
    // Publish the frame's chrome geometry — the model hit-testing reads.
    game.layout.set(crate::layout::LayoutModel::compute(
        vec2(screen_width(), screen_height()),
        s,
        panel_top,
        minimap_rect(game),
        idle_badge,
        cards,
        card_count,
        queue_slots,
        queue_count,
    ));

    // Controls hint — it lives in the same bottom band as the selection
    // panel, so it yields whenever a panel is up (the panel carries its
    // own key prompts).
    if !panel_shown {
        use crate::action::{Action, BindingMap};
        let label = |a: Action| {
            input
                .bindings
                .chord_for(a)
                .map(BindingMap::chord_label)
                .unwrap_or_else(|| "unbound".to_string())
        };
        // Live chords, not folklore: a rebound key changes the prompt.
        let pans = [
            Action::PanLeft,
            Action::PanRight,
            Action::PanUp,
            Action::PanDown,
        ]
        .map(label);
        let pan = if pans == ["Left", "Right", "Up", "Down"].map(String::from) {
            "arrows pan".to_string()
        } else {
            format!("{}/{}/{}/{} pan", pans[0], pans[1], pans[2], pans[3])
        };
        let hint = format!(
            "LMB select · RMB move/engage · 1-9 train · {} build · {} · Esc menu · {} debug",
            label(Action::ToggleBuildPalette),
            pan,
            label(Action::ToggleOverlay),
        );
        let width = measure_text(&hint, None, (16.0 * s) as u16, 1.0).width;
        draw_text(
            &hint,
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

    // Spectator strip: a foundry-less seat on a living team stays in
    // the match by design — masterless machines finish their orders and
    // the team plays on — but the human deserves to be told the seat
    // has no voice left. Commands still route; the sim rejects them.
    if game.state.result().is_none()
        && !game
            .state
            .buildings()
            .iter()
            .any(|b| b.player == game.human && b.kind == oxide_sim::BuildingKind::Foundry)
    {
        let text = "ELIMINATED — SPECTATING";
        let dims = measure_text(text, None, (24.0 * s) as u16, 1.0);
        let x = (screen_width() - dims.width) * 0.5;
        draw_rectangle(
            x - 12.0 * s,
            40.0 * s,
            dims.width + 24.0 * s,
            30.0 * s,
            PANEL,
        );
        draw_text(text, x, 60.0 * s, 24.0 * s, DANGER);
    }

    // Endgame banner.
    if let Some(result) = game.state.result() {
        // The human's verdict first — the game knows whose screen this
        // is; "FERROUS WINS" made every ending read like someone else's.
        let winners = game.state.winners();
        let (text, color) = match result {
            GameResult::Victory { .. } if winners.contains(&game.human) => {
                ("VICTORY".to_string(), SCRAP_COLOR)
            }
            GameResult::Victory { .. } => ("DEFEAT".to_string(), DANGER),
            GameResult::Draw => ("MUTUAL DESTRUCTION".to_string(), BONE_FAINT),
        };
        let sub = match result {
            GameResult::Victory { .. } => {
                let names: Vec<String> = winners
                    .into_iter()
                    .map(|p| game.state.player(p).name.to_uppercase())
                    .collect();
                format!("{} take the field", names.join(" & "))
            }
            GameResult::Draw => "no foundry survived".to_string(),
        };
        let size = 56.0 * s;
        let dims = measure_text(&text, None, size as u16, 1.0);
        let x = (screen_width() - dims.width) * 0.5;
        let y = screen_height() * 0.4;
        draw_rectangle(
            x - 24.0 * s,
            y - 48.0 * s,
            dims.width + 48.0 * s,
            124.0 * s,
            PANEL,
        );
        draw_text(&text, x, y, size, color);
        let sub_dims = measure_text(&sub, None, (20.0 * s) as u16, 1.0);
        draw_text(
            &sub,
            (screen_width() - sub_dims.width) * 0.5,
            y + 26.0 * s,
            20.0 * s,
            BONE_FAINT,
        );
        // The match in numbers: one line per seat from the recomputed
        // record — losses and the peak army it ever fielded — then the
        // army curves themselves, seat-colored, so the shape of the
        // game (the swing, the collapse, the long grind) reads at a
        // glance.
        if let Some(stats) = &game.end_stats {
            let curves_y = y + (92.0 + 22.0 * stats.players.len() as f32) * s;
            let (gw, gh) = (360.0 * s, 96.0 * s);
            let gx = (screen_width() - gw) * 0.5;
            draw_rectangle(
                gx - 8.0 * s,
                curves_y - 8.0 * s,
                gw + 16.0 * s,
                gh + 16.0 * s,
                PANEL,
            );
            let top = stats
                .players
                .iter()
                .flat_map(|p| p.army_value.iter().copied())
                .max()
                .unwrap_or(1)
                .max(1) as f32;
            for (i, seat) in stats.players.iter().enumerate() {
                let faction = game
                    .state
                    .players()
                    .get(i)
                    .map(|p| p.faction)
                    .unwrap_or(oxide_sim::Faction::Ferrous);
                let color = mini_faction_color(faction);
                let n = seat.army_value.len().max(2);
                let mut prev: Option<macroquad::prelude::Vec2> = None;
                for (k, &v) in seat.army_value.iter().enumerate() {
                    let px = gx + gw * k as f32 / (n - 1) as f32;
                    let py = curves_y + gh - gh * (v as f32 / top);
                    let point = vec2(px, py);
                    if let Some(a) = prev {
                        draw_line(a.x, a.y, point.x, point.y, 1.5 * s, color);
                    }
                    prev = Some(point);
                }
            }
            let cap = "army value over the match";
            let cap_dims = measure_text(cap, None, (13.0 * s) as u16, 1.0);
            draw_text(
                cap,
                (screen_width() - cap_dims.width) * 0.5,
                curves_y + gh + 14.0 * s,
                13.0 * s,
                BONE_FAINT,
            );
            for (i, seat) in stats.players.iter().enumerate() {
                let name = game
                    .state
                    .players()
                    .get(i)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| format!("seat {i}"));
                let peak = seat.army_value.iter().copied().max().unwrap_or(0);
                let line = format!(
                    "{name}: lost {} units, {} buildings · peak army {peak} · scrap {}",
                    seat.units_lost,
                    seat.buildings_lost,
                    seat.scrap.last().copied().unwrap_or(0),
                );
                let dims = measure_text(&line, None, (16.0 * s) as u16, 1.0);
                draw_text(
                    &line,
                    (screen_width() - dims.width) * 0.5,
                    y + (86.0 + 22.0 * i as f32) * s,
                    16.0 * s,
                    BONE_FAINT,
                );
            }
        }
        let hint = "Esc — menu";
        let hint_dims = measure_text(hint, None, (20.0 * s) as u16, 1.0);
        draw_text(
            hint,
            (screen_width() - hint_dims.width) * 0.5,
            y + 52.0 * s,
            20.0 * s,
            BONE_FAINT,
        );
    }
}

// --- Minimap ------------------------------------------------------------

const MINIMAP_MAX: Vec2 = vec2(220.0, 150.0);
const MINI_VOID: Color = color_u8!(10, 10, 13, 255);
const MINI_GROUND: Color = color_u8!(44, 44, 52, 255);
const MINI_ROCK: Color = color_u8!(84, 84, 96, 255);
const MINI_PEAK: Color = color_u8!(108, 104, 126, 255);

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
    // The *published* rect, not a recomputation — hit-testing reads the
    // LayoutModel like all chrome, and never touches the window (which
    // also keeps the whole click path headless-testable).
    minimap_world_in(game.layout.get().minimap, game.state.map().width(), screen)
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
/// The panel's clickable geometry: cards, card count, queue slots,
/// queue count, band top.
type PanelGeometry = (
    [(Rect, crate::panel::CardAction); 16],
    usize,
    [(Rect, crate::panel::CardAction); 8],
    usize,
    f32,
);

/// Draws the command panel band and returns its clickable geometry.
fn draw_panel(
    game: &Game,
    sprites: &Sprites,
    input: &InputState,
    panel: &crate::panel::Panel,
) -> PanelGeometry {
    use crate::panel::{CardAction, CardIcon};
    let s = ui_scale();
    let top = screen_height() - 128.0 * s;
    let mini = minimap_rect(game);
    let right = if mini.w > 0.0 {
        (mini.x - 8.0 * s).max(300.0 * s)
    } else {
        screen_width()
    };
    draw_rectangle(0.0, top, right, 128.0 * s, PANEL);
    draw_rectangle(0.0, top, right, 1.5 * s, Color::new(0.6, 0.6, 0.65, 0.4));

    let faction = game.state.player(game.human).faction;
    let icon_source = |icon: &CardIcon| match icon {
        CardIcon::Unit(kind) => Some(sprites.unit(*kind, faction)),
        CardIcon::Building(kind) => Some(sprites.building(*kind, faction)),
        CardIcon::Glyph(_) => None,
    };

    // Portrait block: sprite, name, status.
    let psize = 56.0 * s;
    if let Some(source) = icon_source(&panel.portrait) {
        draw_texture_ex(
            sprites.texture(),
            12.0 * s,
            top + 12.0 * s,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(psize, psize)),
                source: Some(source),
                ..Default::default()
            },
        );
    }
    draw_text(&panel.title, 12.0 * s, top + 88.0 * s, 17.0 * s, BONE);
    draw_text(&panel.sub, 12.0 * s, top + 106.0 * s, 14.0 * s, BONE_FAINT);

    // Command cards: one row of up to eight buttons.
    let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut cards = [(zero, CardAction::None); 16];
    let mut card_count = 0;
    let (cw, ch, gap) = (52.0 * s, 62.0 * s, 6.0 * s);
    let cards_x = 150.0 * s;
    for (i, card) in panel.cards.iter().take(8).enumerate() {
        let rect = Rect::new(cards_x + i as f32 * (cw + gap), top + 10.0 * s, cw, ch);
        if rect.x + rect.w > right {
            break;
        }
        let hovered = rect.contains(input.mouse);
        let bg = if hovered && card.enabled {
            Color::new(0.28, 0.28, 0.33, 1.0)
        } else {
            Color::new(0.16, 0.16, 0.20, 1.0)
        };
        draw_rectangle(rect.x, rect.y, rect.w, rect.h, bg);
        let border = if !card.enabled {
            Color::new(0.4, 0.4, 0.45, 0.5)
        } else if hovered {
            BONE
        } else {
            Color::new(0.55, 0.55, 0.62, 0.9)
        };
        draw_rectangle_lines(rect.x, rect.y, rect.w, rect.h, 1.5 * s, border);
        let tint = if card.enabled {
            WHITE
        } else {
            Color::new(1.0, 1.0, 1.0, 0.35)
        };
        match &card.icon {
            CardIcon::Glyph(g) => {
                let dims = measure_text(g, None, (26.0 * s) as u16, 1.0);
                draw_text(
                    g,
                    rect.x + (rect.w - dims.width) * 0.5,
                    rect.y + 30.0 * s,
                    26.0 * s,
                    if card.enabled { BONE } else { BONE_FAINT },
                );
            }
            icon => {
                if let Some(source) = icon_source(icon) {
                    let isz = 34.0 * s;
                    draw_texture_ex(
                        sprites.texture(),
                        rect.x + (rect.w - isz) * 0.5,
                        rect.y + 4.0 * s,
                        tint,
                        DrawTextureParams {
                            dest_size: Some(vec2(isz, isz)),
                            source: Some(source),
                            ..Default::default()
                        },
                    );
                }
            }
        }
        if let Some(cost) = card.cost {
            let label = format!("{cost}");
            let dims = measure_text(&label, None, (13.0 * s) as u16, 1.0);
            draw_text(
                &label,
                rect.x + (rect.w - dims.width) * 0.5,
                rect.y + rect.h - 5.0 * s,
                13.0 * s,
                if card.enabled {
                    SCRAP_COLOR
                } else {
                    BONE_FAINT
                },
            );
        }
        if !card.hotkey.is_empty() {
            draw_text(
                &card.hotkey,
                rect.x + 3.0 * s,
                rect.y + 12.0 * s,
                11.0 * s,
                BONE_FAINT,
            );
        }
        cards[card_count] = (
            rect,
            if card.enabled {
                card.action
            } else {
                CardAction::None
            },
        );
        card_count += 1;
    }

    // Queue strip along the bottom: production ghosts or order chips.
    let mut queue_slots = [(zero, CardAction::None); 8];
    let mut queue_count = 0;
    if !panel.queue.is_empty() {
        draw_text(
            panel.queue_label,
            150.0 * s,
            top + 92.0 * s,
            13.0 * s,
            BONE_FAINT,
        );
        let (qw, qgap) = (34.0 * s, 4.0 * s);
        let qx0 = 205.0 * s;
        for (i, card) in panel.queue.iter().take(8).enumerate() {
            let rect = Rect::new(qx0 + i as f32 * (qw + qgap), top + 82.0 * s, qw, qw);
            if rect.x + rect.w > right {
                break;
            }
            let hovered = rect.contains(input.mouse);
            draw_rectangle(
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                Color::new(0.14, 0.14, 0.18, 1.0),
            );
            draw_rectangle_lines(
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                1.2 * s,
                if hovered {
                    BONE
                } else {
                    Color::new(0.45, 0.45, 0.52, 0.8)
                },
            );
            match &card.icon {
                CardIcon::Glyph(g) => {
                    let dims = measure_text(g, None, (18.0 * s) as u16, 1.0);
                    draw_text(
                        g,
                        rect.x + (rect.w - dims.width) * 0.5,
                        rect.y + 22.0 * s,
                        18.0 * s,
                        BONE,
                    );
                }
                icon => {
                    if let Some(source) = icon_source(icon) {
                        let isz = 26.0 * s;
                        draw_texture_ex(
                            sprites.texture(),
                            rect.x + (rect.w - isz) * 0.5,
                            rect.y + 4.0 * s,
                            WHITE,
                            DrawTextureParams {
                                dest_size: Some(vec2(isz, isz)),
                                source: Some(source),
                                ..Default::default()
                            },
                        );
                    }
                }
            }
            // The head of a production queue wears its progress.
            if i == 0
                && let Some(bid) = game.selection.building
                && let Some(building) = game.state.building(bid)
                && let Some(&kind) = building.queue.front()
            {
                let total = kind.stats().train_ticks.max(1);
                let frac = (building.progress as f32 / total as f32).clamp(0.0, 1.0);
                draw_rectangle(
                    rect.x,
                    rect.y + rect.h - 3.0 * s,
                    rect.w * frac,
                    3.0 * s,
                    SCRAP_COLOR,
                );
            }
            queue_slots[queue_count] = (rect, card.action);
            queue_count += 1;
        }
    }
    (cards, card_count, queue_slots, queue_count, top)
}

/// Where the tutorial card's dismiss box sits this frame — pure
/// geometry shared by drawing and the click test.
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
        &format!(
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

/// The hover tooltip for panel cards, drawn over everything: name,
/// hotkey, cost, description, weapon lines, and why a disabled card
/// refuses. Rebuilt from the same panel model the frame drew.
fn draw_panel_tooltip(game: &Game, input: &InputState) {
    let Some(panel) = crate::panel::build(game, &input.bindings) else {
        return;
    };
    let layout = game.layout.get();
    if !layout.panel_top.is_finite() {
        return;
    }
    let s = ui_scale();
    let hovered = layout.cards[..layout.card_count]
        .iter()
        .enumerate()
        .find(|(_, (r, _))| r.w > 0.0 && r.contains(input.mouse))
        .and_then(|(i, _)| panel.cards.get(i))
        .or_else(|| {
            layout.queue_slots[..layout.queue_count]
                .iter()
                .enumerate()
                .find(|(_, (r, _))| r.w > 0.0 && r.contains(input.mouse))
                .and_then(|(i, _)| panel.queue.get(i))
        });
    let Some(card) = hovered else {
        return;
    };
    let mut lines: Vec<(String, Color)> = Vec::new();
    let header = if card.hotkey.is_empty() {
        card.title.clone()
    } else {
        format!("{}   [{}]", card.title, card.hotkey)
    };
    lines.push((header, BONE));
    if let Some(cost) = card.cost {
        lines.push((format!("{cost} scrap"), SCRAP_COLOR));
    }
    for d in &card.desc {
        lines.push((d.clone(), BONE_FAINT));
    }
    if let Some(why) = &card.why {
        lines.push((why.clone(), DANGER));
    }
    let size = 15.0 * s;
    let pad = 8.0 * s;
    let width = lines
        .iter()
        .map(|(l, _)| measure_text(l, None, size as u16, 1.0).width)
        .fold(0.0f32, f32::max)
        + pad * 2.0;
    let line_h = 18.0 * s;
    let height = lines.len() as f32 * line_h + pad * 1.5;
    let x = input.mouse.x.min(screen_width() - width - 4.0 * s).max(0.0);
    let y = layout.panel_top - height - 6.0 * s;
    draw_rectangle(x, y, width, height, Color::from_rgba(12, 12, 16, 240));
    draw_rectangle_lines(
        x,
        y,
        width,
        height,
        1.2 * s,
        Color::new(0.55, 0.55, 0.62, 0.9),
    );
    for (i, (line, color)) in lines.iter().enumerate() {
        draw_text(
            line,
            x + pad,
            y + pad + (i as f32 + 0.6) * line_h,
            size,
            *color,
        );
    }
}

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
                (oxide_sim::map::Terrain::Peak, _) => MINI_PEAK,
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

    // Under-attack pulses: an expanding, fading ring where trouble is —
    // or, damped, a steady marker that fades without expanding.
    for (world, age) in &game.alerts {
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
        draw_circle_lines(
            center.x,
            center.y,
            radius,
            1.5,
            Color::new(0.85, 0.32, 0.29, alpha),
        );
    }
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
