//! Everything that lives on the ground: buildings, units, effects,
//! pings, range rings, radar blips, rally lines, breadcrumbs, the
//! placement ghost, and the drag rectangle.

use super::*;

/// The armed building follows the cursor as a translucent footprint —
/// the tint and the command share the shell's queue-aware placement
/// verdict, so what looks legal is legal. Three states: green founds
/// this instant, amber founds on arrival (part of the footprint is
/// remembered ground, judged from memory — never live state, so the
/// tint can't be a hidden-enemy detector), red is refused.
pub(crate) fn draw_placement_ghost(game: &Game, sprites: &Sprites, input: &InputState) {
    let Some(kind) = input.placing else { return };
    let world = game.camera.to_world(input.mouse);
    let anchor = TilePos::new(world.x.floor() as i32, world.y.floor() as i32);
    let zoom = game.camera.zoom;
    let (w, h) = kind.stats().size;
    let queue = input.placing_stroke.is_some() || input.resolver.shift_held();
    let ok = crate::input::placement_refusal(game, kind, anchor, queue).is_none();
    let screen = game
        .camera
        .to_screen(vec2(anchor.x as f32, anchor.y as f32));
    let dest = vec2(w as f32 * zoom, h as f32 * zoom);
    let faction = game.state.player(game.human).faction;
    let tint = if !ok {
        Color::new(1.0, 0.45, 0.4, 0.55)
    } else if crate::input::build_defer_needed(game, kind, anchor) {
        Color::new(1.0, 0.85, 0.45, 0.55)
    } else {
        Color::new(0.7, 1.0, 0.75, 0.55)
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
    if let Some(source) = sprites.defense_mount(kind, faction) {
        draw_texture_ex(
            sprites.texture(),
            screen.x,
            screen.y,
            tint,
            DrawTextureParams {
                dest_size: Some(dest),
                source: Some(source),
                ..Default::default()
            },
        );
    }
}

/// Every deferred claim the human's crews are walking out to, drawn as
/// a faint amber footprint on its promised ground — the promise made
/// visible, deduplicated per (kind, anchor) across the crew.
pub(crate) fn draw_pending_founds(game: &Game, sprites: &Sprites) {
    let zoom = game.camera.zoom;
    let faction = game.state.player(game.human).faction;
    let mut drawn: Vec<(oxide_sim::BuildingKind, TilePos)> = Vec::new();
    for unit in game.state.units().iter().filter(|u| u.player == game.human) {
        for order in std::iter::once(&unit.order).chain(unit.queue.iter()) {
            let oxide_sim::Order::Found { kind, anchor } = order else {
                continue;
            };
            if drawn.contains(&(*kind, *anchor)) {
                continue;
            }
            drawn.push((*kind, *anchor));
            let (w, h) = kind.stats().size;
            let screen = game
                .camera
                .to_screen(vec2(anchor.x as f32, anchor.y as f32));
            draw_texture_ex(
                sprites.texture(),
                screen.x,
                screen.y,
                Color::new(1.0, 0.85, 0.45, 0.3),
                DrawTextureParams {
                    dest_size: Some(vec2(w as f32 * zoom, h as f32 * zoom)),
                    source: Some(sprites.building(*kind, faction)),
                    ..Default::default()
                },
            );
            if let Some(source) = sprites.defense_mount(*kind, faction) {
                draw_texture_ex(
                    sprites.texture(),
                    screen.x,
                    screen.y,
                    Color::new(1.0, 0.85, 0.45, 0.3),
                    DrawTextureParams {
                        dest_size: Some(vec2(w as f32 * zoom, h as f32 * zoom)),
                        source: Some(source),
                        ..Default::default()
                    },
                );
            }
        }
    }
}

/// Queued waypoints of the selection, drawn as a faint chain; a patrol
/// closes the loop. While arming a patrol (`R`), the collected route
/// draws in scrap-amber instead.
/// The screen-space waypoints one selected unit's program draws — pure,
/// so the fog rules are testable: a FOREIGN unit yields no points at
/// all (an ally's or enemy's order chain is intent the viewer has no
/// license to read — fog holds positions, never plans), and own goals
/// draw only on explored ground (the harvest brain can retarget to a
/// node the player has never seen). Each verb speaks its own color:
/// bone walks, danger fights, scrap-gold harvests, patina builds,
/// welds, and strips.
pub(crate) fn breadcrumb_points(game: &Game, unit: &oxide_sim::Unit) -> Vec<(usize, Vec2, Color)> {
    if unit.player != game.human {
        return Vec::new();
    }
    let verb_color = |order: &oxide_sim::Order| match order {
        oxide_sim::Order::Move { .. } => BONE_FAINT,
        oxide_sim::Order::Advance { .. } => Color::new(0.95, 0.76, 0.28, 0.62),
        // A chase and a march are different promises: the chase burns
        // crimson at its victim, the fighting march runs ember toward
        // ground.
        oxide_sim::Order::Attack { .. } => Color::new(0.85, 0.32, 0.29, 0.55),
        oxide_sim::Order::AttackMove { .. } => Color::new(0.88, 0.55, 0.26, 0.55),
        oxide_sim::Order::Harvest { .. } => Color::new(0.85, 0.64, 0.25, 0.55),
        oxide_sim::Order::Build { .. }
        | oxide_sim::Order::Repair { .. }
        | oxide_sim::Order::Salvage { .. }
        | oxide_sim::Order::RepairUnit { .. }
        | oxide_sim::Order::Found { .. } => Color::new(0.25, 0.58, 0.51, 0.55),
        oxide_sim::Order::Idle => BONE_FAINT,
    };
    let goal_of = |order: &oxide_sim::Order| {
        let goal = match order {
            oxide_sim::Order::Move { goal }
            | oxide_sim::Order::Advance { goal }
            | oxide_sim::Order::AttackMove { goal } => *goal,
            oxide_sim::Order::Harvest { node, .. } => *node,
            oxide_sim::Order::Build { site } => game.state.building(*site)?.anchor,
            oxide_sim::Order::Found { anchor, .. } => *anchor,
            oxide_sim::Order::Repair { building } | oxide_sim::Order::Salvage { building } => {
                game.state.building(*building)?.anchor
            }
            // A weld patient is the viewer's own machine — always seen.
            oxide_sim::Order::RepairUnit { unit } => game.state.unit(*unit)?.tile(),
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
        (game.all_seeing() || game.my_vision().explored(goal)).then_some((goal, verb_color(order)))
    };
    // Each point carries its PROGRAM position (0 = the active order,
    // i = queue[i-1]) — the same order the dock pushes chips in, so a
    // fogged leg leaves a numbering gap instead of renumbering the
    // rest out of agreement with the chips.
    let mut points: Vec<(usize, Vec2, Color)> = Vec::new();
    for (i, order) in std::iter::once(&unit.order)
        .chain(unit.queue.iter())
        .enumerate()
    {
        if let Some((g, c)) = goal_of(order) {
            points.push((
                i,
                game.camera
                    .to_screen(vec2(g.x as f32 + 0.5, g.y as f32 + 0.5)),
                c,
            ));
        }
    }
    points
}

/// The selected units that wear decor, SUBJECT FIRST. The dock, the
/// portrait, and the full-strength trail tell ONE unit's story, so the
/// subject can never be the entry the cap drops: a selection arrives
/// in id order, and twelve older workers ahead of a newer majority
/// would push it past `DECOR_CAP`.
pub(crate) fn decor_units(game: &Game) -> Vec<oxide_sim::UnitId> {
    let subject = crate::panel::subject_unit(game);
    let mut ids: Vec<oxide_sim::UnitId> = subject.into_iter().collect();
    ids.extend(
        game.selection
            .units
            .iter()
            .copied()
            .filter(|id| Some(*id) != subject),
    );
    ids.truncate(DECOR_CAP);
    ids
}

pub(crate) fn draw_breadcrumbs(game: &Game, input: &InputState) {
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
    // The dock tells ONE unit's story; the world agrees: the subject's
    // trail draws full strength and numbered, the rest of the
    // selection's trails dim to context.
    let subject = crate::panel::subject_unit(game);
    for id in decor_units(game) {
        let Some(unit) = game.state.unit(id) else {
            continue;
        };
        let points = breadcrumb_points(game, unit);
        if points.is_empty() {
            continue;
        }
        let is_subject = subject == Some(unit.id);
        let fade = |c: Color| {
            if is_subject {
                c
            } else {
                Color::new(c.r, c.g, c.b, c.a * 0.35)
            }
        };
        let start = game
            .camera
            .to_screen(vec2(unit.pos.x.to_num::<f32>(), unit.pos.y.to_num::<f32>()));
        let s = ui_scale();
        // Numbered by PROGRAM position, not by how many survived the
        // fog filter — a fogged leg leaves a gap, it never renumbers
        // the rest away from the dock's chips.
        let numbered = is_subject && !unit.queue.is_empty();
        let mut prev = start;
        for (idx, p, color) in &points {
            let color = fade(*color);
            draw_line(prev.x, prev.y, p.x, p.y, 1.0, color);
            dot(*p, color);
            if numbered {
                draw_text(
                    format!("{}", idx + 1),
                    p.x + 6.0 * s,
                    p.y - 4.0 * s,
                    14.0 * s,
                    color,
                );
            }
            prev = *p;
        }
        // A patrol is a circuit: close it.
        if unit.looping && points.len() > 1 {
            let (_, first, color) = points[0];
            let color = fade(color);
            draw_line(prev.x, prev.y, first.x, first.y, 1.0, color);
        }
    }
}

fn production_progress_visible(game: &Game, building: &oxide_sim::Building) -> bool {
    building.player == game.human || game.all_seeing()
}

fn draw_defense_mount(
    game: &Game,
    sprites: &Sprites,
    building: &oxide_sim::Building,
    action: Option<usize>,
) {
    let faction = game.state.player(building.player).faction;
    let screen = game
        .camera
        .to_screen(vec2(building.anchor.x as f32, building.anchor.y as f32));
    let (width, height) = building.kind.stats().size;
    let dest = vec2(
        width as f32 * game.camera.zoom,
        height as f32 * game.camera.zoom,
    );
    let source = match action {
        Some(frame) => sprites.defense_mount_action(building.kind, faction, frame),
        None => sprites.defense_mount(building.kind, faction),
    };
    let Some(source) = source else {
        return;
    };
    let angle = game
        .aim_buildings
        .get(&building.id.0)
        .map_or(0.0, |(angle, _)| *angle);
    draw_texture_ex(
        sprites.texture(),
        screen.x,
        screen.y,
        WHITE,
        DrawTextureParams {
            dest_size: Some(dest),
            source: Some(source),
            rotation: angle,
            ..Default::default()
        },
    );
    let accent_source = match action {
        Some(frame) => sprites.defense_mount_action_accent(building.kind, frame),
        None => sprites.defense_mount_accent(building.kind),
    };
    if let (Some(accent), Some(source)) = (seat_identity_tint(game, building.player), accent_source)
    {
        draw_texture_ex(
            sprites.texture(),
            screen.x,
            screen.y,
            accent,
            DrawTextureParams {
                dest_size: Some(dest),
                source: Some(source),
                rotation: angle,
                ..Default::default()
            },
        );
    }
}

fn draw_bastion_charge_overlay(
    game: &Game,
    sprites: &Sprites,
    building: &oxide_sim::Building,
    action: Option<usize>,
) {
    let faction = game.state.player(building.player).faction;
    let (source, placement) = sprites.bastion_charge_overlay(faction, action);
    let screen = game
        .camera
        .to_screen(vec2(building.anchor.x as f32, building.anchor.y as f32));
    let (width, height) = building.kind.stats().size;
    let footprint = vec2(
        width as f32 * game.camera.zoom,
        height as f32 * game.camera.zoom,
    );
    draw_texture_ex(
        sprites.texture(),
        screen.x + footprint.x * placement.x,
        screen.y + footprint.y * placement.y,
        WHITE,
        DrawTextureParams {
            dest_size: Some(vec2(footprint.x * placement.w, footprint.y * placement.h)),
            source: Some(source),
            ..Default::default()
        },
    );
}

pub(crate) fn draw_buildings(game: &Game, sprites: &Sprites) {
    let zoom = game.camera.zoom;
    // Buildings an own crew is actively stripping (the salvage
    // read-back's fog-safe evidence).
    let salvaging: Vec<oxide_sim::BuildingId> = game
        .state
        .units()
        .iter()
        .filter(|u| u.player == game.human)
        .filter_map(|u| match u.order {
            oxide_sim::Order::Salvage { building } => Some(building),
            _ => None,
        })
        .collect();
    // Live enemy buildings only where we have sight; remembered ghosts
    // cover explored-but-unseen ground (skipped in the omniscient overlay).
    if !game.all_seeing() {
        for ghost in game.my_vision().ghosts() {
            let (w, h) = ghost.kind.stats().size;
            let visible = (0..h)
                .flat_map(|dy| (0..w).map(move |dx| ghost.anchor.offset(dx, dy)))
                .any(|t| game.my_vision().visible(t));
            let key = (ghost.anchor.x, ghost.anchor.y);
            if visible {
                game.last_seen.borrow_mut().insert(key, game.fx_time());
                continue; // the live building (or its absence) is on show
            }
            // Staleness ramp: a memory the player has not refreshed in
            // a while stops pretending to be news. Unstamped memories
            // (loaded saves) start their ramp now.
            let age = {
                let mut seen = game.last_seen.borrow_mut();
                let stamp = *seen.entry(key).or_insert_with(|| game.fx_time());
                game.fx_time() - stamp
            };
            let fade = 1.0 - super::staleness_fade(age);
            let faction = game.state.player(ghost.owner).faction;
            let screen = game
                .camera
                .to_screen(vec2(ghost.anchor.x as f32, ghost.anchor.y as f32));
            // A remembered site stays translucent scaffolding until its
            // completion has actually been observed.
            let tint = if ghost.built {
                Color::new(
                    GHOST_TINT.r,
                    GHOST_TINT.g,
                    GHOST_TINT.b,
                    GHOST_TINT.a * fade,
                )
            } else {
                Color::new(
                    GHOST_TINT.r,
                    GHOST_TINT.g,
                    GHOST_TINT.b,
                    GHOST_TINT.a * 0.5 * fade,
                )
            };
            // The memory keeps its allegiance accent at the memory's
            // own alpha: a translucent own-faction sprite is also how
            // the player's own construction sites draw, and a memory
            // must never masquerade as one of those.
            let accent_tint =
                seat_identity_tint(game, ghost.owner).map(|c| Color::new(c.r, c.g, c.b, tint.a));
            let dest = vec2(w as f32 * zoom, h as f32 * zoom);
            let mut layers = vec![(sprites.building(ghost.kind, faction), tint)];
            if let Some(accent) = accent_tint {
                layers.push((sprites.building_accent(ghost.kind), accent));
            }
            if ghost.built
                && let Some(mount) = sprites.defense_mount(ghost.kind, faction)
            {
                // Defense bases ship bare; memories retain a static,
                // north-facing silhouette without inventing live aim.
                layers.push((mount, tint));
                if let Some(accent) = accent_tint {
                    layers.push((
                        sprites
                            .defense_mount_accent(ghost.kind)
                            .expect("a defense mount has an accent"),
                        accent,
                    ));
                }
            }
            for (source, color) in layers {
                draw_texture_ex(
                    sprites.texture(),
                    screen.x,
                    screen.y,
                    color,
                    DrawTextureParams {
                        dest_size: Some(dest),
                        source: Some(source),
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
        let animation = game.animations.building_state(
            crate::presentation_animation::BuildingAnimationFacts::capture(&game.state, building),
            crate::presentation_animation::AnimationClock::from_state(
                &game.state,
                game.tick_fraction(),
            ),
            crate::presentation_animation::AnimationOptions {
                reduced_motion: reduced_motion(),
            },
        );
        let frame = super::motion::building_frame(building.kind, animation);
        let (source, accent_source) = match frame.body {
            super::motion::BuildingBodyFrame::Idle => (
                sprites.building(building.kind, faction),
                sprites.building_accent(building.kind),
            ),
            super::motion::BuildingBodyFrame::Work(work) => (
                sprites.building_working(building.kind, faction, work + 1),
                sprites.building_working_accent(building.kind, work + 1),
            ),
            super::motion::BuildingBodyFrame::Construction { stage, phase } => (
                sprites.construction(building.kind, faction, stage, phase),
                sprites.construction_accent(building.kind, stage, phase),
            ),
            super::motion::BuildingBodyFrame::Action(action) => (
                sprites.building_action(building.kind, faction, action),
                sprites.building_action_accent(building.kind, action),
            ),
        };
        draw_texture_ex(
            sprites.texture(),
            screen.x,
            screen.y,
            WHITE,
            DrawTextureParams {
                dest_size: Some(dest),
                source: Some(source),
                ..Default::default()
            },
        );
        let accent_tint = seat_identity_tint(game, building.player);
        if let Some(accent) = accent_tint {
            draw_texture_ex(
                sprites.texture(),
                screen.x,
                screen.y,
                accent,
                DrawTextureParams {
                    dest_size: Some(dest),
                    source: Some(accent_source),
                    ..Default::default()
                },
            );
        }
        if building.built {
            match building.kind {
                oxide_sim::BuildingKind::Turret
                | oxide_sim::BuildingKind::FlakTurret
                | oxide_sim::BuildingKind::Bastion => {
                    draw_defense_mount(game, sprites, building, frame.mount_action);
                    if building.kind == oxide_sim::BuildingKind::Bastion {
                        draw_bastion_charge_overlay(game, sprites, building, frame.mount_action);
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
        if game.selection.buildings.contains(&building.id) {
            draw_rectangle_lines(
                screen.x - 2.0,
                screen.y - 2.0,
                dest.x + 4.0,
                dest.y + 4.0,
                3.0,
                BONE,
            );
        }
        // One bar per story: a site's partial hp is what the ramp
        // GRANTS, so the progress bar tells it alone — the hp bar
        // joins only when fire has taken hp construction already gave.
        // The check mirrors the sim's integer ramp exactly (a float
        // restatement flickers), and gates on !built because progress
        // doubles as the train counter on finished producers.
        let max_hp = building.kind.stats().max_hp;
        let under_own_salvage = building.built && salvaging.contains(&building.id);
        let wounded = if under_own_salvage {
            // The gold teardown bar below carries the fraction; a
            // second bar restating it in hp colors is the double-bar
            // disease this pass exists to cure.
            false
        } else if building.built {
            building.hp < max_hp
        } else {
            let ticks = building
                .kind
                .stats()
                .construction
                .map(|c| c.build_ticks)
                .unwrap_or(1);
            let start = max_hp / 5;
            let expected = start + (max_hp - start) * building.progress.min(ticks) / ticks;
            building.hp < expected
        };
        if wounded {
            hp_bar(screen.x, screen.y - 8.0, dest.x, building.hp, max_hp);
        }
        // Production progress, drawn under the works.
        if production_progress_visible(game, building)
            && let Some(kind) = building.queue.front()
        {
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
        // A teardown in progress: gold — the scrap coming back — over
        // remaining substance. Keyed on an OWN crew's Order::Salvage,
        // never on hp shape (shelling looks identical), so enemy
        // salvage shows nothing through the fog.
        if under_own_salvage {
            let fraction = building.hp as f32 / max_hp as f32;
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

pub(crate) fn draw_units(game: &Game, sprites: &Sprites, alpha: f32) {
    // Two passes: ground bodies first, then everything airborne above
    // them — each flyer casts an offset shadow so altitude reads even
    // when nothing overlaps.
    draw_unit_pass(game, sprites, alpha, oxide_sim::stats::Domain::Ground);
    draw_unit_pass(game, sprites, alpha, oxide_sim::stats::Domain::Air);
}

fn shell_visual_origin(launch: Vec2, impact: Vec2, shooter: oxide_sim::Target) -> Vec2 {
    let direction = impact - launch;
    if direction.length_squared() <= f32::EPSILON {
        return launch;
    }
    let reach = match shooter {
        oxide_sim::Target::Unit(_) => 0.46,
        oxide_sim::Target::Building(_) => {
            oxide_sim::BuildingKind::Bastion.stats().size.0 as f32 * 0.49
        }
    };
    launch + direction.normalize() * reach
}

fn shell_arc_lift(screen_distance: f32, zoom: f32, shooter: oxide_sim::Target) -> f32 {
    match shooter {
        // Bombard remains visibly indirect artillery, but never throws
        // its shell more than three-fifths of a tile above the flat path.
        oxide_sim::Target::Unit(_) => (screen_distance * 0.06).min(zoom * 0.60),
        // Bastion is a low-carriage siege gun: its old moonshot made the
        // compact shell look detached from the barrel and impact.
        oxide_sim::Target::Building(_) => (screen_distance * 0.04).min(zoom * 0.40),
    }
}

fn shell_tail_start(progress: f32, world_distance: f32) -> f32 {
    if world_distance <= f32::EPSILON {
        return progress;
    }
    (progress - 0.34 / world_distance).max(0.0)
}

fn terminal_segment(from: Vec2, to: Vec2, length: f32) -> (Vec2, Vec2) {
    let path = to - from;
    let distance = path.length();
    if distance <= f32::EPSILON {
        return (to, to);
    }
    (to - path / distance * length.min(distance), to)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShotVisibility {
    Hidden,
    ImpactOnly,
    Full,
}

fn shot_visibility(
    style: crate::game::ShotStyle,
    source_visible: bool,
    impact_visible: bool,
) -> ShotVisibility {
    if !impact_visible {
        ShotVisibility::Hidden
    } else if style == crate::game::ShotStyle::Contact || !source_visible {
        ShotVisibility::ImpactOnly
    } else {
        ShotVisibility::Full
    }
}

pub(crate) fn draw_fx(game: &Game, sprites: &Sprites) {
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
        let launch = vec2(
            shell.launch.x.to_num::<f32>(),
            shell.launch.y.to_num::<f32>(),
        );
        let to = vec2(
            shell.impact.x.to_num::<f32>(),
            shell.impact.y.to_num::<f32>(),
        );
        // Indirect building fire currently means Bastion fire. Its sim
        // launch stays at the stable footprint center; presentation
        // advances that point to the authored barrel mouth.
        let from = shell_visual_origin(launch, to, shell.shooter);
        // Fog rule: own and allied shells draw throughout their flight;
        // a hostile shell appears only while its current local segment
        // crosses visible ground. Nothing anchors a trail at a fogged
        // muzzle and pinpoints the hidden artillery.
        let mine = !game.state.hostile(game.human, shell.player);
        let flat_seen = |k: f32| sees(from.lerp(to, k));
        // Reconstruct flight length the way the launch computed it, so
        // the shell lands exactly when the sim resolves the hit.
        let total = (launch.distance(to) / shell_speed).ceil().max(1.0);
        let elapsed = total - (shell.arrival as f32 - now);
        let t = (elapsed / total).clamp(0.0, 1.0);
        if !game.all_seeing() && !mine && !flat_seen(t) {
            continue;
        }
        let a = game.camera.to_screen(from);
        let b = game.camera.to_screen(to);
        let dist = (b - a).length();
        let lift = shell_arc_lift(dist, game.camera.zoom, shell.shooter);
        let at = |t: f32| {
            let flat = a.lerp(b, t);
            vec2(flat.x, flat.y - lift * 4.0 * t * (1.0 - t))
        };
        let tail_t = shell_tail_start(t, from.distance(to));
        let tail = at(tail_t);
        let shell_at = at(t);
        let flat = a.lerp(b, t);
        let radius = (game.camera.zoom * 0.075).clamp(2.2, 4.0);
        // The tiny flat-path shadow makes the restrained lift legible
        // without restoring the old launch-to-impact glowing arc.
        draw_circle(
            flat.x,
            flat.y,
            radius * 0.7,
            Color::new(0.03, 0.03, 0.04, 0.35),
        );
        if game.all_seeing() || mine || flat_seen(tail_t) {
            draw_line(
                tail.x,
                tail.y,
                shell_at.x,
                shell_at.y,
                radius * 1.15,
                Color::new(0.18, 0.16, 0.15, 0.95),
            );
            let hot_tail = tail.lerp(shell_at, 0.55);
            draw_line(
                hot_tail.x,
                hot_tail.y,
                shell_at.x,
                shell_at.y,
                radius * 0.42,
                Color::new(0.95, 0.65, 0.32, 0.72),
            );
        }
        draw_circle(
            shell_at.x,
            shell_at.y,
            radius,
            Color::new(0.12, 0.12, 0.14, 1.0),
        );
        draw_circle(
            shell_at.x,
            shell_at.y,
            radius * 0.42,
            Color::new(0.92, 0.86, 0.72, 1.0),
        );
    }
    for fx in &game.fx {
        // A visible impact may always spark so incoming damage reads.
        // Directional geometry still requires a visible source and must
        // not pinpoint a fogged shooter.
        let in_sight = match fx.kind {
            EffectKind::DirectShot { style, from, to } => {
                shot_visibility(style, sees(from), sees(to)) != ShotVisibility::Hidden
            }
            EffectKind::Puff { at } => sees(at),
            EffectKind::Falling { at, .. } => sees(at),
            EffectKind::Burst { at, .. } => sees(at),
            EffectKind::Debris { at, .. } => sees(at),
            // Own-order acknowledgments always show; fogged targets are
            // already impossible to order onto.
            EffectKind::Ping { .. } => true,
        };
        if !game.all_seeing() && !in_sight {
            continue;
        }
        match fx.kind {
            EffectKind::DirectShot { style, from, to } => {
                use crate::game::ShotStyle;
                let a = game.camera.to_screen(from);
                let b = game.camera.to_screen(to);
                let progress = (fx.age / style.life()).clamp(0.0, 1.0);
                let fade = 1.0 - progress;
                let visibility = shot_visibility(
                    style,
                    game.all_seeing() || sees(from),
                    game.all_seeing() || sees(to),
                );
                if visibility == ShotVisibility::ImpactOnly {
                    let seed = (to.x * 31.7 + to.y * 17.3).abs();
                    for i in 0..3 {
                        let angle = seed + i as f32 * 2.1;
                        let reach = game.camera.zoom * (0.08 + progress * 0.12);
                        let tip = b + vec2(angle.cos(), angle.sin()) * reach;
                        draw_line(
                            b.x,
                            b.y,
                            tip.x,
                            tip.y,
                            1.5,
                            Color::new(0.95, 0.67, 0.34, fade),
                        );
                    }
                    continue;
                }
                match style {
                    ShotStyle::Contact => {}
                    ShotStyle::Tracer => {
                        let length = game.camera.zoom * 0.34 * (0.75 + fade * 0.25);
                        let (start, end) = terminal_segment(a, b, length);
                        draw_line(
                            start.x,
                            start.y,
                            end.x,
                            end.y,
                            3.2,
                            Color::new(0.12, 0.11, 0.11, 0.85 * fade),
                        );
                        draw_line(
                            start.x,
                            start.y,
                            end.x,
                            end.y,
                            1.2,
                            Color::new(0.98, 0.80, 0.52, fade),
                        );
                    }
                    ShotStyle::Rail => {
                        draw_line(
                            a.x,
                            a.y,
                            b.x,
                            b.y,
                            10.0 * fade.max(0.25),
                            Color::new(0.64, 0.78, 0.96, 0.24 * fade),
                        );
                        draw_line(
                            a.x,
                            a.y,
                            b.x,
                            b.y,
                            3.5 * fade.max(0.3),
                            Color::new(0.92, 0.96, 1.0, fade),
                        );
                    }
                    ShotStyle::FlakBurst => {
                        let direction = (b - a).normalize_or_zero();
                        let normal = vec2(-direction.y, direction.x);
                        for (side, stagger) in [(-1.0, 0.12), (1.0, 0.0)] {
                            let end = b + normal * side * game.camera.zoom * 0.055
                                - direction * game.camera.zoom * stagger;
                            let (start, end) = terminal_segment(
                                a + normal * side * game.camera.zoom * 0.055,
                                end,
                                game.camera.zoom * 0.32,
                            );
                            draw_line(
                                start.x,
                                start.y,
                                end.x,
                                end.y,
                                3.0,
                                Color::new(0.10, 0.10, 0.10, 0.82 * fade),
                            );
                            draw_line(
                                start.x,
                                start.y,
                                end.x,
                                end.y,
                                1.1,
                                Color::new(0.90, 0.86, 0.70, fade),
                            );
                        }
                        let seed = (to.x * 31.7 + to.y * 17.3).abs();
                        for i in 0..3 {
                            let angle = seed + i as f32 * 2.1;
                            let reach = progress * game.camera.zoom * 0.28;
                            let puff = b + vec2(angle.cos(), angle.sin()) * reach;
                            draw_circle(
                                puff.x,
                                puff.y,
                                game.camera.zoom * 0.07 * (1.0 - progress * 0.45),
                                Color::new(0.66, 0.65, 0.58, 0.42 * fade),
                            );
                        }
                    }
                    ShotStyle::HeavyRound => {
                        let (start, end) = terminal_segment(a, b, game.camera.zoom * 0.48);
                        draw_line(
                            start.x,
                            start.y,
                            end.x,
                            end.y,
                            5.0,
                            Color::new(0.10, 0.09, 0.09, 0.9 * fade),
                        );
                        draw_line(
                            start.lerp(end, 0.52).x,
                            start.lerp(end, 0.52).y,
                            end.x,
                            end.y,
                            1.7,
                            Color::new(0.94, 0.62, 0.30, 0.8 * fade),
                        );
                        draw_circle(end.x, end.y, 2.4, Color::new(0.13, 0.12, 0.12, fade));
                    }
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
            EffectKind::Debris { at, seed } => {
                // Three shards on seed-derived arcs: radial fling that
                // decays, a gravity-flavored settle, spin, and a fade.
                // Everything derives from (seed, i), so a replay draws
                // the same scatter the live session did.
                let t = (fx.age / 0.7).clamp(0.0, 1.0);
                let zoom = game.camera.zoom;
                for i in 0..3u32 {
                    let h = seed
                        .wrapping_mul(2_654_435_761)
                        .wrapping_add(i.wrapping_mul(40_503))
                        .rotate_left(13);
                    let angle = (h % 628) as f32 / 100.0;
                    let fling = 0.55 + ((h >> 10) % 60) as f32 / 100.0;
                    let reach = fling * (1.0 - (1.0 - t) * (1.0 - t));
                    let world = vec2(
                        at.x + angle.cos() * reach,
                        at.y + angle.sin() * reach + t * t * 0.35,
                    );
                    let p = game.camera.to_screen(world);
                    let size = zoom * 0.34 * (1.0 - t * 0.4);
                    draw_texture_ex(
                        sprites.texture(),
                        p.x - size * 0.5,
                        p.y - size * 0.5,
                        Color::new(1.0, 1.0, 1.0, 1.0 - t),
                        DrawTextureParams {
                            dest_size: Some(vec2(size, size)),
                            source: Some(sprites.debris(i as usize)),
                            rotation: angle + t * 4.0,
                            ..Default::default()
                        },
                    );
                }
            }
            EffectKind::Ping { .. } => {} // drawn above the fog, in draw_pings
        }
    }
}

/// Radar blips, drawn above the fog: contacts without identity from the
/// Array's outer ring — the player's own intel, like pings.
pub(crate) fn draw_blips(game: &Game) {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuildingRangeKind {
    Weapon,
    DeadZone,
    Vision,
    Radar,
    Repair,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum BuildingRangeShape {
    Circle { center: Vec2, radius: f32 },
    FootprintOffset { min: Vec2, max: Vec2, radius: f32 },
}

impl BuildingRangeShape {
    #[cfg(test)]
    fn outer_bounds(self) -> (Vec2, Vec2) {
        match self {
            Self::Circle { center, radius } => {
                let reach = vec2(radius, radius);
                (center - reach, center + reach)
            }
            Self::FootprintOffset { min, max, radius } => {
                let reach = vec2(radius, radius);
                (min - reach, max + reach)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct BuildingRange {
    kind: BuildingRangeKind,
    shape: BuildingRangeShape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RangeStroke {
    Solid,
    ShortDash,
    LongDash,
    Dotted,
    DashDot,
}

impl RangeStroke {
    fn visible(self, distance: f32, scale: f32) -> bool {
        let scale = scale.max(0.25);
        match self {
            Self::Solid => true,
            Self::ShortDash => (distance / scale).rem_euclid(12.0) < 6.0,
            Self::LongDash => (distance / scale).rem_euclid(20.0) < 12.0,
            Self::Dotted => (distance / scale).rem_euclid(9.0) < 2.5,
            Self::DashDot => {
                let phase = (distance / scale).rem_euclid(26.0);
                phase < 11.0 || (16.0..19.0).contains(&phase)
            }
        }
    }
}

fn range_stroke(kind: BuildingRangeKind) -> RangeStroke {
    match kind {
        BuildingRangeKind::Weapon => RangeStroke::Solid,
        BuildingRangeKind::DeadZone => RangeStroke::ShortDash,
        BuildingRangeKind::Vision => RangeStroke::LongDash,
        BuildingRangeKind::Radar => RangeStroke::Dotted,
        BuildingRangeKind::Repair => RangeStroke::DashDot,
    }
}

fn range_icon(kind: BuildingRangeKind) -> crate::panel::CombatIcon {
    use crate::panel::CombatIcon;
    match kind {
        BuildingRangeKind::Weapon => CombatIcon::Weapon,
        BuildingRangeKind::DeadZone => CombatIcon::DeadZone,
        BuildingRangeKind::Vision => CombatIcon::Vision,
        BuildingRangeKind::Radar => CombatIcon::Radar,
        BuildingRangeKind::Repair => CombatIcon::Repair,
    }
}

fn dead_zone_fill(color: Color) -> Color {
    Color::new(color.r, color.g, color.b, 0.055)
}

fn range_subject(
    units: &[oxide_sim::UnitId],
    buildings: &[oxide_sim::BuildingId],
) -> Option<oxide_sim::Target> {
    match (units, buildings) {
        ([unit], []) => Some(oxide_sim::Target::Unit(*unit)),
        ([], [building]) => Some(oxide_sim::Target::Building(*building)),
        _ => None,
    }
}

fn stroke_patterned_path(
    points: &[Vec2],
    stroke: RangeStroke,
    thickness: f32,
    color: Color,
    scale: f32,
) {
    let mut traveled = 0.0;
    let sample = (3.0 * scale).max(1.5);
    for pair in points.windows(2) {
        let delta = pair[1] - pair[0];
        let length = delta.length();
        if length <= f32::EPSILON {
            continue;
        }
        let steps = (length / sample).ceil().max(1.0) as usize;
        for step in 0..steps {
            let a_t = step as f32 / steps as f32;
            let b_t = (step + 1) as f32 / steps as f32;
            let midpoint = traveled + length * (a_t + b_t) * 0.5;
            if stroke.visible(midpoint, scale) {
                let a = pair[0] + delta * a_t;
                let b = pair[0] + delta * b_t;
                draw_line(a.x, a.y, b.x, b.y, thickness, color);
            }
        }
        traveled += length;
    }
}

fn circle_path(center: Vec2, radius: f32) -> Vec<Vec2> {
    let segments = ((std::f32::consts::TAU * radius / 4.0).ceil() as usize).clamp(48, 240);
    (0..=segments)
        .map(|segment| {
            let angle = std::f32::consts::TAU * segment as f32 / segments as f32;
            center + vec2(angle.cos(), angle.sin()) * radius
        })
        .collect()
}

fn rounded_footprint_path(min: Vec2, max: Vec2, radius: f32) -> Vec<Vec2> {
    const CORNER_SEGMENTS: usize = 12;
    let outer_min = min - vec2(radius, radius);
    let outer_max = max + vec2(radius, radius);
    let mut points = Vec::with_capacity(4 * (CORNER_SEGMENTS + 2) + 1);
    points.push(vec2(min.x, outer_min.y));
    points.push(vec2(max.x, outer_min.y));
    for (center, start) in [
        (vec2(max.x, min.y), -std::f32::consts::FRAC_PI_2),
        (max, 0.0),
        (vec2(min.x, max.y), std::f32::consts::FRAC_PI_2),
        (min, std::f32::consts::PI),
    ] {
        for segment in 1..=CORNER_SEGMENTS {
            let angle =
                start + std::f32::consts::FRAC_PI_2 * segment as f32 / CORNER_SEGMENTS as f32;
            points.push(center + vec2(angle.cos(), angle.sin()) * radius);
        }
        let next = match start {
            value if value < 0.0 => vec2(outer_max.x, max.y),
            0.0 => vec2(min.x, outer_max.y),
            value if value < std::f32::consts::PI => vec2(outer_min.x, min.y),
            _ => points[0],
        };
        points.push(next);
    }
    points
}

/// One range vocabulary serves selected buildings and placement ghosts.
/// Keeping the visit pure makes omissions testable without a graphics
/// context and avoids allocating in the frame loop.
fn visit_building_ranges(
    anchor: Vec2,
    kind: oxide_sim::BuildingKind,
    mut visit: impl FnMut(BuildingRange),
) {
    let stats = kind.stats();
    let size = vec2(stats.size.0 as f32, stats.size.1 as f32);
    let center = anchor + size * 0.5;
    if let Some(weapon) = stats.weapons.first() {
        visit(BuildingRange {
            kind: BuildingRangeKind::Weapon,
            shape: BuildingRangeShape::Circle {
                center,
                radius: weapon.range.to_num::<f32>(),
            },
        });
        if weapon.minimum_range > chassis::fx::Fx::ZERO {
            visit(BuildingRange {
                kind: BuildingRangeKind::DeadZone,
                shape: BuildingRangeShape::Circle {
                    center,
                    radius: weapon.minimum_range.to_num::<f32>(),
                },
            });
        }
        if weapon.range.to_num::<f32>() > stats.vision as f32 {
            visit(BuildingRange {
                kind: BuildingRangeKind::Vision,
                shape: BuildingRangeShape::Circle {
                    center,
                    radius: stats.vision as f32,
                },
            });
        }
    }
    if kind == oxide_sim::BuildingKind::Array {
        visit(BuildingRange {
            kind: BuildingRangeKind::Vision,
            shape: BuildingRangeShape::Circle {
                center,
                radius: stats.vision as f32,
            },
        });
        visit(BuildingRange {
            kind: BuildingRangeKind::Radar,
            shape: BuildingRangeShape::Circle {
                center,
                radius: oxide_sim::stats::RADAR_DETECT_RADIUS as f32,
            },
        });
    }
    if kind == oxide_sim::BuildingKind::RepairBay {
        visit(BuildingRange {
            kind: BuildingRangeKind::Repair,
            shape: BuildingRangeShape::FootprintOffset {
                min: anchor,
                max: anchor + size,
                radius: oxide_sim::stats::REPAIR_BAY_RADIUS.to_num::<f32>(),
            },
        });
    }
}

pub(crate) fn draw_range_rings(game: &Game, input: &InputState) {
    let s = ui_scale();
    let ring = |world: Vec2,
                radius: f32,
                stroke: RangeStroke,
                icon: crate::panel::CombatIcon,
                color: Color,
                thickness: f32| {
        if radius <= 0.0 {
            return;
        }
        let center = game.camera.to_screen(world);
        let screen_radius = radius * game.camera.zoom;
        if stroke == RangeStroke::Solid {
            draw_circle_lines(center.x, center.y, screen_radius, thickness * s, color);
        } else {
            stroke_patterned_path(
                &circle_path(center, screen_radius),
                stroke,
                thickness * s,
                color,
                s,
            );
        }
        let glyph = center + vec2(screen_radius * 0.707, -screen_radius * 0.707);
        draw_combat_icon(glyph, 5.6 * s, icon, color, true);
    };
    let footprint_offset = |min: Vec2,
                            max: Vec2,
                            radius: f32,
                            stroke: RangeStroke,
                            icon: crate::panel::CombatIcon,
                            color: Color| {
        if radius <= 0.0 {
            return;
        }
        let min = game.camera.to_screen(min);
        let max = game.camera.to_screen(max);
        let radius = radius * game.camera.zoom;
        stroke_patterned_path(
            &rounded_footprint_path(min, max, radius),
            stroke,
            1.7 * s,
            color,
            s,
        );
        draw_combat_icon(
            vec2(max.x + radius * 0.707, min.y - radius * 0.707),
            5.6 * s,
            icon,
            color,
            true,
        );
    };
    let weapon_color = Color::new(0.85, 0.32, 0.29, 0.55);
    let dead_zone_color = Color::new(1.0, 0.68, 0.18, 0.78);
    let air_weapon_color = Color::new(0.38, 0.70, 0.95, 0.52);
    let vision_color = Color::new(0.63, 0.77, 0.94, 0.42);
    let radar_color = Color::new(0.22, 0.76, 0.72, 0.52);
    let repair_color = Color::new(0.38, 0.82, 0.45, 0.55);

    let unit_rings = |world: Vec2, stats: &oxide_sim::stats::UnitStats| {
        for weapon in stats.weapons {
            let icon = crate::panel::weapon_combat_icon(weapon);
            let color = if icon == crate::panel::CombatIcon::AirWeapon {
                air_weapon_color
            } else {
                weapon_color
            };
            ring(
                world,
                weapon.range.to_num::<f32>(),
                RangeStroke::Solid,
                icon,
                color,
                1.7,
            );
        }
        // Guns past their own eyes need a spotter: show the gap.
        if stats
            .weapons
            .iter()
            .any(|w| w.range.to_num::<f32>() > stats.vision as f32)
        {
            ring(
                world,
                stats.vision as f32,
                RangeStroke::LongDash,
                crate::panel::CombatIcon::Vision,
                vision_color,
                1.7,
            );
        }
    };
    let building_rings = |anchor: Vec2, kind: oxide_sim::BuildingKind| {
        visit_building_ranges(anchor, kind, |range| {
            let color = match range.kind {
                BuildingRangeKind::Weapon => weapon_color,
                BuildingRangeKind::DeadZone => dead_zone_color,
                BuildingRangeKind::Vision => vision_color,
                BuildingRangeKind::Radar => radar_color,
                BuildingRangeKind::Repair => repair_color,
            };
            let stroke = range_stroke(range.kind);
            let icon = range_icon(range.kind);
            match range.shape {
                BuildingRangeShape::Circle { center, radius } => {
                    if range.kind == BuildingRangeKind::DeadZone {
                        let center = game.camera.to_screen(center);
                        draw_circle(
                            center.x,
                            center.y,
                            radius * game.camera.zoom,
                            dead_zone_fill(dead_zone_color),
                        );
                    }
                    let thickness = if range.kind == BuildingRangeKind::DeadZone {
                        2.2
                    } else {
                        1.7
                    };
                    ring(center, radius, stroke, icon, color, thickness);
                }
                BuildingRangeShape::FootprintOffset { min, max, radius } => {
                    footprint_offset(min, max, radius, stroke, icon, color);
                }
            }
        });
    };

    match range_subject(&game.selection.units, &game.selection.buildings) {
        Some(oxide_sim::Target::Unit(id)) => {
            if let Some(unit) = game.state.unit(id)
                && unit.player == game.human
            {
                let world = vec2(unit.pos.x.to_num::<f32>(), unit.pos.y.to_num::<f32>());
                unit_rings(world, unit.kind.stats());
            }
        }
        Some(oxide_sim::Target::Building(id)) => {
            if let Some(building) = game.state.building(id) {
                building_rings(
                    vec2(building.anchor.x as f32, building.anchor.y as f32),
                    building.kind,
                );
            }
        }
        None => {}
    }
    // The armed placement ghost carries its rings to the cursor.
    if let Some(kind) = input.placing {
        let world = game.camera.to_world(input.mouse);
        let anchor = vec2(world.x.floor(), world.y.floor());
        building_rings(anchor, kind);
    }
}

pub(crate) fn draw_pings(game: &Game) {
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
pub(crate) fn draw_rally_marker(game: &Game) {
    // A selected producer draws the line to its rally, not just the
    // flag — where fresh machines will walk should read at a glance.
    // OWN producers only, like the flag below: the foreign panel hides
    // rally and orders on purpose, and an inspected enemy building
    // must not leak its intent through this line either.
    for (building, rally) in game
        .selection
        .buildings
        .iter()
        .filter_map(|id| game.state.building(*id))
        .filter(|building| building.player == game.human)
        .filter_map(|building| building.rally.map(|rally| (building, rally)))
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
    for rally in game
        .selection
        .buildings
        .iter()
        .filter_map(|id| game.state.building(*id))
        .filter(|building| building.player == game.human)
        .filter_map(|building| building.rally)
    {
        draw_rally_flag(game, rally, game.camera.zoom);
    }
}

pub(crate) fn draw_drag_rect(game: &Game, input: &InputState) {
    let Some(origin) = input.drag_origin else {
        return;
    };
    let now = input.mouse;
    let feedback = crate::input::drag_feedback(origin, now, ui_scale());
    if feedback == crate::input::DragFeedback::Still {
        return;
    }
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
    if feedback != crate::input::DragFeedback::Selection {
        return;
    }
    // Live preview starts only once release would commit a box-select.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_rings_have_one_clear_subject() {
        let unit = oxide_sim::UnitId(3);
        let building = oxide_sim::BuildingId(5);
        assert_eq!(
            range_subject(&[unit], &[]),
            Some(oxide_sim::Target::Unit(unit))
        );
        assert_eq!(
            range_subject(&[], &[building]),
            Some(oxide_sim::Target::Building(building))
        );
        assert_eq!(range_subject(&[unit, oxide_sim::UnitId(4)], &[]), None);
        assert_eq!(range_subject(&[unit], &[building]), None);
    }

    #[test]
    fn production_progress_is_private_except_in_an_omniscient_view() {
        let mut game = Game::with_viewport(oxide_sim::Scenario::skirmish(), vec2(1280.0, 800.0))
            .expect("embedded skirmish builds");
        let own = game
            .state
            .buildings()
            .iter()
            .find(|building| building.player == game.human)
            .expect("the human has a Foundry");
        let hostile = game
            .state
            .buildings()
            .iter()
            .find(|building| building.player != game.human)
            .expect("the opponent has a Foundry");
        assert!(production_progress_visible(&game, own));
        assert!(!production_progress_visible(&game, hostile));

        game.spectate = true;
        assert!(production_progress_visible(&game, hostile));
    }

    #[test]
    fn repair_bay_uses_the_exact_footprint_offset_aura() {
        let anchor = vec2(10.0, 20.0);
        let mut ranges = Vec::new();
        visit_building_ranges(anchor, oxide_sim::BuildingKind::RepairBay, |range| {
            ranges.push(range);
        });

        let [range] = ranges.as_slice() else {
            panic!("Repair Bay should expose exactly one range: {ranges:?}");
        };
        assert_eq!(range.kind, BuildingRangeKind::Repair);
        let radius = oxide_sim::stats::REPAIR_BAY_RADIUS.to_num::<f32>();
        let size = oxide_sim::BuildingKind::RepairBay.stats().size;
        let footprint_max = anchor + vec2(size.0 as f32, size.1 as f32);
        assert_eq!(
            range.shape,
            BuildingRangeShape::FootprintOffset {
                min: anchor,
                max: footprint_max,
                radius,
            }
        );
        assert_eq!(
            range.shape.outer_bounds(),
            (
                anchor - vec2(radius, radius),
                footprint_max + vec2(radius, radius),
            )
        );
    }

    #[test]
    fn weapon_ranges_remain_centered_circles() {
        let anchor = vec2(10.0, 20.0);
        let kind = oxide_sim::BuildingKind::Turret;
        let mut ranges = Vec::new();
        visit_building_ranges(anchor, kind, |range| ranges.push(range));

        let weapon = ranges
            .iter()
            .find(|range| range.kind == BuildingRangeKind::Weapon)
            .expect("a Turret exposes its weapon range");
        let size = kind.stats().size;
        assert_eq!(
            weapon.shape,
            BuildingRangeShape::Circle {
                center: anchor + vec2(size.0 as f32, size.1 as f32) * 0.5,
                radius: kind.stats().weapons[0].range.to_num::<f32>(),
            }
        );
        assert!(
            ranges
                .iter()
                .all(|range| range.kind != BuildingRangeKind::DeadZone),
            "zero-minimum-range weapons must not invent a warning circle"
        );
    }

    #[test]
    fn bastion_dead_zone_is_a_shaded_inner_circle() {
        let anchor = vec2(10.0, 20.0);
        let kind = oxide_sim::BuildingKind::Bastion;
        let mut ranges = Vec::new();
        visit_building_ranges(anchor, kind, |range| ranges.push(range));

        let dead_zone = ranges
            .iter()
            .find(|range| range.kind == BuildingRangeKind::DeadZone)
            .expect("a Bastion exposes its close-pressure counter");
        let size = kind.stats().size;
        assert_eq!(
            dead_zone.shape,
            BuildingRangeShape::Circle {
                center: anchor + vec2(size.0 as f32, size.1 as f32) * 0.5,
                radius: kind.stats().weapons[0].minimum_range.to_num::<f32>(),
            }
        );
        assert_eq!(
            range_stroke(dead_zone.kind),
            RangeStroke::ShortDash,
            "the inner boundary cannot read as another solid weapon radius"
        );
        let line = Color::new(1.0, 0.68, 0.18, 0.78);
        let fill = dead_zone_fill(line);
        assert_eq!((fill.r, fill.g, fill.b), (line.r, line.g, line.b));
        assert!(
            fill.a > 0.0 && fill.a < 0.1,
            "the static wash must read without obscuring units: {fill:?}"
        );
    }

    #[test]
    fn every_range_meaning_has_its_own_line_texture() {
        let kinds = [
            BuildingRangeKind::Weapon,
            BuildingRangeKind::DeadZone,
            BuildingRangeKind::Vision,
            BuildingRangeKind::Radar,
            BuildingRangeKind::Repair,
        ];
        let strokes = kinds.map(range_stroke);
        for (index, stroke) in strokes.iter().enumerate() {
            assert!(
                strokes[..index].iter().all(|other| other != stroke),
                "{stroke:?} was reused for two range meanings"
            );
        }

        for stroke in [
            RangeStroke::ShortDash,
            RangeStroke::LongDash,
            RangeStroke::Dotted,
            RangeStroke::DashDot,
        ] {
            for scale in [0.75, 1.0, 2.0] {
                let samples: Vec<bool> = (0..240)
                    .map(|sample| stroke.visible(sample as f32 * scale * 0.5, scale))
                    .collect();
                assert!(samples.iter().any(|visible| *visible));
                assert!(
                    samples.iter().any(|visible| !*visible),
                    "{stroke:?} became solid at {scale}x"
                );
            }
        }

        let icons = kinds.map(range_icon);
        for (index, icon) in icons.iter().enumerate() {
            assert!(
                icons[..index].iter().all(|other| other != icon),
                "{icon:?} was reused for two range meanings"
            );
        }
    }

    #[test]
    fn artillery_shells_begin_at_the_barrel_and_use_a_low_arc() {
        let launch = vec2(5.0, 7.0);
        let impact = vec2(15.0, 7.0);
        let from = shell_visual_origin(
            launch,
            impact,
            oxide_sim::Target::Building(oxide_sim::BuildingId(4)),
        );
        assert!((from.x - 5.98).abs() < 1.0e-4);
        assert_eq!(from.y, launch.y);
        let bombard_from = shell_visual_origin(
            launch,
            impact,
            oxide_sim::Target::Unit(oxide_sim::UnitId(4)),
        );
        assert!((bombard_from.x - 5.46).abs() < 1.0e-4);
        assert_eq!(bombard_from.y, launch.y);
        assert_eq!(
            shell_visual_origin(
                launch,
                launch,
                oxide_sim::Target::Unit(oxide_sim::UnitId(4))
            ),
            launch
        );

        let zoom = 32.0;
        let bombard = oxide_sim::Target::Unit(oxide_sim::UnitId(4));
        let bastion = oxide_sim::Target::Building(oxide_sim::BuildingId(4));
        assert!((shell_arc_lift(320.0, zoom, bombard) - 19.2).abs() < 1.0e-4);
        assert!((shell_arc_lift(320.0, zoom, bastion) - 12.8).abs() < 1.0e-4);
        assert_eq!(shell_arc_lift(1_000.0, zoom, bombard), zoom * 0.60);
        assert_eq!(shell_arc_lift(1_000.0, zoom, bastion), zoom * 0.40);

        let tail = shell_tail_start(0.5, 10.0);
        assert!((tail - 0.466).abs() < 1.0e-4);
        assert!((0.5 - tail) * 10.0 <= 0.340_001);
    }

    #[test]
    fn physical_direct_shots_use_short_terminal_segments() {
        let (start, end) = terminal_segment(vec2(0.0, 0.0), vec2(100.0, 0.0), 12.0);
        assert_eq!(start, vec2(88.0, 0.0));
        assert_eq!(end, vec2(100.0, 0.0));

        let (start, end) = terminal_segment(vec2(4.0, 8.0), vec2(4.0, 8.0), 12.0);
        assert_eq!(start, end);
    }

    #[test]
    fn shot_visibility_never_points_into_fog() {
        use crate::game::ShotStyle;

        assert_eq!(
            shot_visibility(ShotStyle::Rail, true, true),
            ShotVisibility::Full
        );
        assert_eq!(
            shot_visibility(ShotStyle::Tracer, false, true),
            ShotVisibility::ImpactOnly
        );
        assert_eq!(
            shot_visibility(ShotStyle::Contact, true, true),
            ShotVisibility::ImpactOnly
        );
        assert_eq!(
            shot_visibility(ShotStyle::HeavyRound, true, false),
            ShotVisibility::Hidden
        );
    }

    #[test]
    fn patterned_paths_close_on_their_authored_outer_bounds() {
        let center = vec2(30.0, 40.0);
        let circle = circle_path(center, 12.0);
        assert!((circle[0] - circle[circle.len() - 1]).length() < 1.0e-4);

        let min = vec2(10.0, 20.0);
        let max = vec2(24.0, 31.0);
        let radius = 7.0;
        let footprint = rounded_footprint_path(min, max, radius);
        assert!((footprint[0] - footprint[footprint.len() - 1]).length() < 1.0e-4);
        let low = footprint
            .iter()
            .fold(vec2(f32::INFINITY, f32::INFINITY), |bound, point| {
                bound.min(*point)
            });
        let high = footprint.iter().fold(
            vec2(f32::NEG_INFINITY, f32::NEG_INFINITY),
            |bound, point| bound.max(*point),
        );
        assert!((low - (min - vec2(radius, radius))).length() < 1.0e-4);
        assert!((high - (max + vec2(radius, radius))).length() < 1.0e-4);
    }
}
