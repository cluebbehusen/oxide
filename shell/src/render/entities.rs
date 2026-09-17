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
pub(crate) fn draw_placement_ghost(
    game: &crate::game::Scene<'_>,
    sprites: &Sprites,
    input: &InputState,
) {
    let Some(kind) = input.placing else { return };
    let world = game.presentation.camera.to_world(input.mouse);
    let clicked = TilePos::new(world.x.floor() as i32, world.y.floor() as i32);
    let anchor = crate::input::placement_anchor(game, kind, clicked);
    let zoom = game.presentation.camera.zoom;
    let (w, h) = kind.base_stats().size;
    let queue = input.placing_stroke.is_some() || input.resolver.shift_held();
    let ok = crate::input::placement_refusal(game, kind, anchor, queue).is_none();
    let screen = game
        .presentation
        .camera
        .to_screen(vec2(anchor.x as f32, anchor.y as f32));
    let dest = vec2(w as f32 * zoom, h as f32 * zoom);
    let faction = game.state.player(game.presentation.human).faction;
    let tint = if !ok {
        Color::new(1.0, 0.45, 0.4, 0.55)
    } else if crate::input::build_defer_needed(game, kind, anchor) {
        Color::new(1.0, 0.85, 0.45, 0.55)
    } else {
        Color::new(0.7, 1.0, 0.75, 0.55)
    };
    sprites.draw(
        screen.x,
        screen.y,
        tint,
        DrawTextureParams {
            dest_size: Some(dest),
            source: Some(sprites.construction(kind, faction, 0, 0)),
            ..Default::default()
        },
    );
}

/// Paid provisional scaffolds remain faint amber footprints until their
/// ground has been verified.
pub(crate) fn draw_pending_founds(game: &crate::game::Scene<'_>, sprites: &Sprites) {
    let zoom = game.presentation.camera.zoom;
    let faction = game.state.player(game.presentation.human).faction;
    for site in game
        .state
        .buildings()
        .iter()
        .filter(|b| b.player == game.presentation.human && b.provisional)
    {
        let (w, h) = site.stats().size;
        let screen = game
            .presentation
            .camera
            .to_screen(vec2(site.anchor.x as f32, site.anchor.y as f32));
        sprites.draw(
            screen.x,
            screen.y,
            Color::new(1.0, 0.85, 0.45, 0.3),
            DrawTextureParams {
                dest_size: Some(vec2(w as f32 * zoom, h as f32 * zoom)),
                source: Some(sprites.construction(site.kind, faction, 0, 0)),
                ..Default::default()
            },
        );
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
pub(crate) fn breadcrumb_points(
    game: &crate::game::Scene<'_>,
    unit: &oxide_sim::Unit,
) -> Vec<(usize, Vec2, Color)> {
    if unit.player != game.presentation.human {
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
        oxide_sim::Order::ReturnCargo { .. } | oxide_sim::Order::Harvest { .. } => {
            Color::new(0.85, 0.64, 0.25, 0.55)
        }
        oxide_sim::Order::Build { .. }
        | oxide_sim::Order::Repair { .. }
        | oxide_sim::Order::Salvage { .. }
        | oxide_sim::Order::RepairUnit { .. }
        | oxide_sim::Order::Found { .. } => Color::new(0.25, 0.58, 0.51, 0.55),
        oxide_sim::Order::Board { .. }
        | oxide_sim::Order::Unload { .. }
        | oxide_sim::Order::Land { .. } => BONE_FAINT,
        oxide_sim::Order::Idle => BONE_FAINT,
    };
    let goal_of = |order: &oxide_sim::Order| {
        let goal = match order {
            oxide_sim::Order::Move { goal }
            | oxide_sim::Order::Advance { goal }
            | oxide_sim::Order::AttackMove { goal } => *goal,
            oxide_sim::Order::Harvest { node, .. } => *node,
            oxide_sim::Order::ReturnCargo { foundry, .. } => game.state.building(*foundry)?.anchor,
            oxide_sim::Order::Build { site } => game.state.building(*site)?.anchor,
            oxide_sim::Order::Found { anchor, .. } => *anchor,
            oxide_sim::Order::Repair { building } | oxide_sim::Order::Salvage { building } => {
                game.state.building(*building)?.anchor
            }
            // A weld patient is the viewer's own machine — always seen.
            oxide_sim::Order::RepairUnit { unit } => game.state.unit(*unit)?.tile(),
            oxide_sim::Order::Board { transport } => game.state.unit(*transport)?.tile(),
            oxide_sim::Order::Unload { at } => *at,
            oxide_sim::Order::Land { goal } => *goal,
            oxide_sim::Order::Attack { target, .. } => {
                let view = game.state.attack_view(game.presentation.human, *target)?;
                return Some((
                    chassis::grid::TilePos::containing(view.position),
                    verb_color(order),
                ));
            }
            oxide_sim::Order::Idle => return None,
        };
        (game.presentation.all_seeing() || game.my_vision().explored(goal))
            .then_some((goal, verb_color(order)))
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
                game.presentation
                    .camera
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
pub(crate) fn decor_units(game: &crate::game::Scene<'_>) -> Vec<oxide_sim::UnitId> {
    let subject = crate::panel::subject_unit(game);
    let mut ids: Vec<oxide_sim::UnitId> = subject.into_iter().collect();
    ids.extend(
        game.presentation
            .selection
            .units
            .iter()
            .copied()
            .filter(|id| Some(*id) != subject),
    );
    ids.truncate(DECOR_CAP);
    ids
}

pub(crate) fn draw_breadcrumbs(game: &crate::game::Scene<'_>, input: &InputState) {
    let dot = |p: Vec2, color: Color| draw_circle(p.x, p.y, 3.0, color);
    if let Some(route) = &input.patrol_route {
        let mut prev: Option<Vec2> = None;
        for tile in route {
            let p = game
                .presentation
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
            .presentation
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

fn production_progress_visible(
    game: &crate::game::Scene<'_>,
    building: &oxide_sim::Building,
) -> bool {
    building.player == game.presentation.human || game.presentation.all_seeing()
}

fn draw_defense_mount(
    game: &crate::game::Scene<'_>,
    sprites: &Sprites,
    building: &oxide_sim::Building,
    action: Option<usize>,
) {
    let draw = |x, y, tint, params| {
        sprites.draw_building(x, y, tint, params, game.presentation.camera.zoom)
    };
    let faction = game.state.player(building.player).faction;
    let screen = game
        .presentation
        .camera
        .to_screen(vec2(building.anchor.x as f32, building.anchor.y as f32));
    let (width, height) = building.stats().size;
    let dest = vec2(
        width as f32 * game.presentation.camera.zoom,
        height as f32 * game.presentation.camera.zoom,
    );
    let source = match action {
        Some(frame) => sprites.defense_mount_action(building.kind, building.tier, faction, frame),
        None => sprites.defense_mount(building.kind, building.tier, faction),
    };
    let Some(source) = source else {
        return;
    };
    let angle = game
        .presentation
        .aim_buildings
        .get(&building.id.0)
        .map_or(0.0, |(angle, _)| *angle);
    draw(
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
        Some(frame) => sprites.defense_mount_action_accent(building.kind, building.tier, frame),
        None => sprites.defense_mount_accent(building.kind, building.tier),
    };
    if let (Some(accent), Some(source)) = (seat_identity_tint(game, building.player), accent_source)
    {
        draw(
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

pub(crate) fn draw_buildings(game: &crate::game::Scene<'_>, sprites: &Sprites) {
    let zoom = game.presentation.camera.zoom;
    let draw = |x, y, tint, params| sprites.draw_building(x, y, tint, params, zoom);
    // Buildings an own crew is actively stripping (the salvage
    // read-back's fog-safe evidence).
    let salvaging: Vec<oxide_sim::BuildingId> = game
        .state
        .units()
        .iter()
        .filter(|u| u.player == game.presentation.human)
        .filter_map(|u| match u.order {
            oxide_sim::Order::Salvage { building } => Some(building),
            _ => None,
        })
        .collect();
    // Live enemy buildings only where we have sight; remembered ghosts
    // cover explored-but-unseen ground (skipped in the omniscient overlay).
    if !game.presentation.all_seeing() {
        for ghost in game.my_vision().ghosts() {
            let (w, h) = ghost.kind.base_stats().size;
            let visible = (0..h)
                .flat_map(|dy| (0..w).map(move |dx| ghost.anchor.offset(dx, dy)))
                .any(|t| game.my_vision().visible(t));
            let key = (ghost.anchor.x, ghost.anchor.y);
            let observed = visible
                && game.state.buildings_at(ghost.anchor).any(|b| {
                    b.player == ghost.owner
                        && b.kind == ghost.kind
                        && game.state.building_apparent(game.presentation.human, b)
                });
            if observed {
                game.presentation
                    .last_seen
                    .borrow_mut()
                    .insert(key, game.presentation.fx_time());
                continue; // the live building (or its absence) is on show
            }
            // Staleness ramp: a memory the player has not refreshed in
            // a while stops pretending to be news. Unstamped memories
            // (loaded saves) start their ramp now.
            let age = {
                let mut seen = game.presentation.last_seen.borrow_mut();
                let stamp = *seen
                    .entry(key)
                    .or_insert_with(|| game.presentation.fx_time());
                game.presentation.fx_time() - stamp
            };
            let fade = 1.0 - super::staleness_fade(age);
            let faction = game.state.player(ghost.owner).faction;
            let screen = game
                .presentation
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
            let body = if ghost.built {
                sprites.building(ghost.kind, faction)
            } else {
                sprites.construction(ghost.kind, faction, 0, 0)
            };
            let body_accent = if ghost.built {
                sprites.building_accent(ghost.kind)
            } else {
                sprites.construction_accent(ghost.kind, 0, 0)
            };
            let mut layers = vec![(body, tint)];
            if let Some(accent) = accent_tint {
                layers.push((body_accent, accent));
            }
            if ghost.built
                && let Some(mount) = sprites.defense_mount(ghost.kind, 0, faction)
            {
                // Defense bases ship bare; memories retain a static,
                // north-facing silhouette without inventing live aim.
                layers.push((mount, tint));
                if let Some(accent) = accent_tint {
                    layers.push((
                        sprites
                            .defense_mount_accent(ghost.kind, 0)
                            .expect("a defense mount has an accent"),
                        accent,
                    ));
                }
            }
            for (source, color) in layers {
                draw(
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
    // Frustum cull by anchor with a margin covering the widest footprint
    // plus bars and site dressing — off-camera works cost nothing.
    let (view_lo, view_hi) = game.presentation.camera.world_rect();
    const BUILDING_CULL_MARGIN: f32 = 4.5;
    for building in game.state.buildings().iter().filter(|b| !b.provisional) {
        if building.player != game.presentation.human
            && !game.presentation.all_seeing()
            && (!building.tiles().any(|t| game.my_vision().visible(t))
                || !game
                    .state
                    .building_apparent(game.presentation.human, building))
        {
            continue;
        }
        let anchor = vec2(building.anchor.x as f32, building.anchor.y as f32);
        if anchor.x < view_lo.x - BUILDING_CULL_MARGIN
            || anchor.y < view_lo.y - BUILDING_CULL_MARGIN
            || anchor.x > view_hi.x + BUILDING_CULL_MARGIN
            || anchor.y > view_hi.y + BUILDING_CULL_MARGIN
        {
            continue;
        }
        let faction = game.state.player(building.player).faction;
        let screen = game.presentation.camera.to_screen(anchor);
        let (w, h) = building.stats().size;
        let dest = vec2(w as f32 * zoom, h as f32 * zoom);
        let animation = game.presentation.animations.building_state(
            crate::presentation_animation::BuildingAnimationFacts::capture(game.state, building),
            crate::presentation_animation::AnimationClock::from_state(
                game.state,
                game.presentation.tick_fraction(),
            ),
            crate::presentation_animation::AnimationOptions {
                reduced_motion: reduced_motion(),
            },
        );
        let frame = super::motion::building_frame(building.kind, animation);
        let array_layers = (building.built && building.kind == oxide_sim::BuildingKind::Array)
            .then(|| sprites.array_rig())
            .flatten()
            .map(|rig| rig.layers(building.tier, faction));
        let (source, accent_source) = array_layers.map_or_else(
            || match frame.body {
                super::motion::BuildingBodyFrame::Idle => (
                    sprites.building_tiered(building.kind, building.tier, faction),
                    sprites.building_tiered_accent(building.kind, building.tier),
                ),
                super::motion::BuildingBodyFrame::Work(work) => (
                    sprites.building_working(building.kind, building.tier, faction, work + 1),
                    sprites.building_working_accent(building.kind, building.tier, work + 1),
                ),
                super::motion::BuildingBodyFrame::Construction { stage, phase } => (
                    sprites.construction(building.kind, faction, stage, phase),
                    sprites.construction_accent(building.kind, stage, phase),
                ),
                super::motion::BuildingBodyFrame::Action(action) => (
                    sprites.building_action(building.kind, faction, action),
                    sprites.building_action_accent(building.kind, action),
                ),
            },
            |layers| layers[0],
        );
        draw(
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
            draw(
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
        if let Some(layers) = array_layers {
            let cycle = match animation.activity {
                crate::presentation_animation::BuildingActivity::ArraySweep { cycle } => cycle,
                _ => 0.0,
            };
            let rotation = 20.0_f32.to_radians() - cycle * std::f32::consts::TAU;
            let pivot = screen + dest * vec2(0.5, 49.0 / 128.0);
            let (source, accent) = layers[1];
            for (source, tint) in
                std::iter::once((source, WHITE)).chain(accent_tint.map(|tint| (accent, tint)))
            {
                draw(
                    screen.x,
                    screen.y,
                    tint,
                    DrawTextureParams {
                        dest_size: Some(dest),
                        source: Some(source),
                        rotation,
                        pivot: Some(pivot),
                        ..Default::default()
                    },
                );
            }
        }
        if building.built {
            match building.kind {
                oxide_sim::BuildingKind::Turret
                | oxide_sim::BuildingKind::FlakTurret
                | oxide_sim::BuildingKind::Bastion => {
                    draw_defense_mount(game, sprites, building, frame.mount_action);
                }
                _ => {}
            }
        }
        if !building.built {
            // Construction progress in bone, distinct from training amber.
            let ticks = building
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
        if game.presentation.selection.buildings.contains(&building.id) {
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
        let max_hp = building.stats().max_hp;
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

pub(crate) fn draw_units(game: &crate::game::Scene<'_>, sprites: &Sprites, alpha: f32) {
    // Two passes: ground bodies first, then everything airborne above
    // them — each flyer casts an offset shadow so altitude reads even
    // when nothing overlaps.
    draw_unit_pass(game, sprites, alpha, oxide_sim::stats::Domain::Ground);
    draw_bomber_bombs(game);
    draw_unit_pass(game, sprites, alpha, oxide_sim::stats::Domain::Air);
}

fn bomber_release(game: &crate::game::Scene<'_>, index: usize) -> Option<crate::game::LaunchPose> {
    game.presentation
        .projectile_releases
        .release(game.state.shells(), index)
        .or_else(|| {
            let shell = game.state.shells().get(index)?;
            if shell.kind != oxide_sim::ProjectileKind::Bomb {
                return None;
            }
            let oxide_sim::Target::Unit(id) = shell.shooter else {
                return None;
            };
            let kind = game.state.unit(id)?.kind;
            if !matches!(
                kind,
                oxide_sim::UnitKind::Condor | oxide_sim::UnitKind::Moth
            ) {
                return None;
            }
            // Seeks without launch history use a stable impact line, never the
            // aircraft's later egress heading.
            Some(crate::game::LaunchPose {
                heading: vec2(
                    (shell.impact.x - shell.launch.x).to_num::<f32>(),
                    (shell.impact.y - shell.launch.y).to_num::<f32>(),
                )
                .normalize_or_zero(),
                kind,
                slot: 0,
            })
        })
}

fn condor_bomb_pose(launch: Vec2, impact: Vec2, heading: Vec2, t: f32) -> (Vec2, Vec2) {
    let t = t.clamp(0.0, 1.0);
    let reach = 0.53_f32.min(launch.distance(impact) * 0.35);
    let start = launch + heading * reach;
    let lead = 0.65_f32.min(start.distance(impact) * 0.4);
    let c1 = start + heading * lead;
    let c2 = impact - (impact - start).normalize_or_zero() * lead;
    let q = 1.0 - t;
    let position = start
        .lerp(c1, t)
        .lerp(c1.lerp(c2, t), t)
        .lerp(c1.lerp(c2, t).lerp(c2.lerp(impact, t), t), t);
    let tangent = (c1 - start) * (q * q) + (c2 - c1) * (2.0 * q * t) + (impact - c2) * (t * t);
    (position, tangent.normalize_or_zero())
}

fn moth_bomb_pose(
    launch: Vec2,
    impact: Vec2,
    release: crate::game::LaunchPose,
    t: f32,
    total: f32,
) -> (Vec2, Vec2) {
    let heading = release.heading;
    let side = vec2(-heading.y, heading.x);
    let row = release.slot / 2;
    let lateral = if release.slot.is_multiple_of(2) {
        -0.234375
    } else {
        0.234375
    };
    let start = launch + side * lateral + heading * ((19.0 - row as f32 * 14.0) / 64.0);
    let lead = (oxide_sim::UnitKind::Moth.stats().speed.to_num::<f32>() * total / 3.0)
        .min(start.distance(impact) * 0.4);
    let c1 = start + heading * lead;
    let c2 = impact - (impact - start).normalize_or_zero() * lead;
    let t = t.clamp(0.0, 1.0);
    let q = 1.0 - t;
    let position = start
        .lerp(c1, t)
        .lerp(c1.lerp(c2, t), t)
        .lerp(c1.lerp(c2, t).lerp(c2.lerp(impact, t), t), t);
    let tangent = (c1 - start) * (q * q) + (c2 - c1) * (2.0 * q * t) + (impact - c2) * (t * t);
    (position, tangent.normalize_or_zero())
}

fn draw_bomber_bombs(game: &crate::game::Scene<'_>) {
    let zoom = game.presentation.camera.zoom;
    let now = game.state.current_tick() as f32 + game.presentation.tick_fraction();
    for (index, shell) in game.state.shells().iter().enumerate() {
        let Some(release) = bomber_release(game, index) else {
            continue;
        };
        let launch = vec2(
            shell.launch.x.to_num::<f32>(),
            shell.launch.y.to_num::<f32>(),
        );
        let impact = vec2(
            shell.impact.x.to_num::<f32>(),
            shell.impact.y.to_num::<f32>(),
        );
        let total = (launch.distance(impact) / oxide_sim::stats::SHELL_SPEED.to_num::<f32>())
            .ceil()
            .max(1.0);
        let t = (1.0 - (shell.arrival as f32 - now) / total).clamp(0.0, 1.0);
        let moth = release.kind == oxide_sim::UnitKind::Moth;
        let (position, direction) = if moth {
            moth_bomb_pose(launch, impact, release, t, total)
        } else {
            condor_bomb_pose(launch, impact, release.heading, t)
        };
        if !game.presentation.all_seeing()
            && game.state.hostile(game.presentation.human, shell.player)
            && !game.my_vision().visible(TilePos::new(
                position.x.floor() as i32,
                position.y.floor() as i32,
            ))
        {
            continue;
        }
        let flat = game.presentation.camera.to_screen(position);
        let lift = if moth { 0.08 } else { 0.0625 };
        let center = flat - vec2(0.0, zoom * lift * (1.0 - t * t));
        let scale = zoom * (1.0 - 0.15 * t) * if moth { 0.70 } else { 1.0 };
        let normal = vec2(-direction.y, direction.x);
        let nose = center + direction * scale * 0.14;
        let back = center - direction * scale * 0.14;
        draw_circle(
            flat.x + zoom * 0.10,
            flat.y + zoom * 0.14,
            scale * 0.065,
            Color::new(0.02, 0.02, 0.03, 0.35),
        );
        draw_line(
            back.x,
            back.y,
            nose.x,
            nose.y,
            scale * 0.15,
            Color::from_rgba(12, 13, 17, 255),
        );
        draw_line(
            back.x,
            back.y,
            nose.x,
            nose.y,
            scale * 0.095,
            Color::from_rgba(151, 146, 134, 255),
        );
        let tip = nose - direction * scale * 0.05;
        draw_triangle(
            nose,
            tip + normal * scale * 0.048,
            tip - normal * scale * 0.048,
            Color::from_rgba(210, 199, 171, 255),
        );
        let fin = back + direction * scale * 0.06;
        draw_triangle(
            back,
            fin + normal * scale * 0.085,
            fin - normal * scale * 0.085,
            Color::from_rgba(92, 74, 59, 255),
        );
    }
}

pub(crate) fn shell_visual_origin(
    launch: Vec2,
    impact: Vec2,
    shooter: oxide_sim::Target,
    kind: oxide_sim::ProjectileKind,
) -> Vec2 {
    if kind == oxide_sim::ProjectileKind::Bomb {
        return launch;
    }
    let direction = impact - launch;
    if direction.length_squared() <= f32::EPSILON {
        return launch;
    }
    let reach = match shooter {
        oxide_sim::Target::Unit(_) => {
            if kind == oxide_sim::ProjectileKind::Missile {
                0.53
            } else {
                31.0 / 128.0 * super::unit_draw_scale(oxide_sim::UnitKind::Bombard)
            }
        }
        oxide_sim::Target::Building(_) => {
            oxide_sim::BuildingKind::Bastion.base_stats().size.0 as f32 * 0.49
        }
    };
    launch + direction.normalize() * reach
}

fn bombard_shell_position(launch: Vec2, impact: Vec2, heading: Vec2, progress: f32) -> Vec2 {
    let t = progress.clamp(0.0, 1.0);
    let direction = (impact - launch).normalize_or_zero();
    let heading = if heading.length_squared() > 0.0 {
        heading.normalize()
    } else {
        direction
    };
    let distance = launch.distance(impact);
    let muzzle = launch
        + heading
            * (31.0 / 128.0 * super::unit_draw_scale(oxide_sim::UnitKind::Bombard))
                .min(distance * 0.4);
    muzzle.lerp(impact, t)
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

fn missile_ejection_ticks(total_ticks: f32) -> f32 {
    crate::audio_timeline::missile_ejection_ticks(total_ticks)
}

pub(crate) fn missile_travel_progress(progress: f32, total_ticks: f32, distance: f32) -> f32 {
    let progress = progress.clamp(0.0, 1.0);
    if distance <= f32::EPSILON {
        return progress;
    }
    let ejection_ticks = missile_ejection_ticks(total_ticks);
    let ejection_distance = 0.45_f32.min(distance * 0.12);
    let ejection_speed = ejection_distance / ejection_ticks;
    let elapsed = progress * total_ticks;
    if elapsed <= ejection_ticks {
        return ejection_speed * elapsed / distance;
    }
    let powered_ticks = total_ticks - ejection_ticks;
    let ramp_ticks = 2.0_f32.min(powered_ticks * 0.25);
    // Integrate the ignition ramp while preserving the sim's arrival tick.
    let cruise_speed = (distance - ejection_distance - ejection_speed * ramp_ticks * 0.5)
        / (powered_ticks - ramp_ticks * 0.5);
    let powered_elapsed = elapsed - ejection_ticks;
    let powered_distance = if powered_elapsed < ramp_ticks {
        ejection_speed * powered_elapsed
            + (cruise_speed - ejection_speed) * powered_elapsed.powi(2) / (2.0 * ramp_ticks)
    } else {
        ejection_speed * ramp_ticks * 0.5 + cruise_speed * (powered_elapsed - ramp_ticks * 0.5)
    };
    ((ejection_distance + powered_distance) / distance).clamp(0.0, 1.0)
}

fn missile_motor_strength(progress: f32, total_ticks: f32) -> f32 {
    let ejection_ticks = missile_ejection_ticks(total_ticks);
    let ramp_ticks = 2.0_f32.min((total_ticks - ejection_ticks) * 0.25);
    ((progress * total_ticks - ejection_ticks) / ramp_ticks).clamp(0.0, 1.0)
}

fn shell_tail_start(progress: f32, world_distance: f32) -> f32 {
    if world_distance <= f32::EPSILON {
        return progress;
    }
    (progress - 0.14 / world_distance).max(0.0)
}

const FLAK_ROUND_TRAVEL: f32 = 0.10;
const FORGE_SPOT_TRAVEL_FRACTION: f32 = 0.60;

fn flak_round_progress(age: f32, yoke_delay: crate::game::FlakYokeDelay) -> [Option<f32>; 2] {
    let round = |delay: f32| {
        let local = age - delay;
        (0.0..=FLAK_ROUND_TRAVEL)
            .contains(&local)
            .then(|| (local / FLAK_ROUND_TRAVEL).clamp(0.0, 1.0))
    };
    [round(0.0), round(yoke_delay.seconds())]
}

fn flak_barrel_rounds(
    age: f32,
    delay: crate::game::FlakYokeDelay,
    count: u8,
) -> [Option<(f32, f32)>; 6] {
    let groups = flak_round_progress(age, delay);
    let count = usize::from(count.clamp(1, 3));
    let offsets = match (count, delay) {
        (3, _) => [
            -31.0 / 128.0,
            -23.0 / 128.0,
            -15.0 / 128.0,
            15.0 / 128.0,
            23.0 / 128.0,
            31.0 / 128.0,
        ],
        (2, crate::game::FlakYokeDelay::OneAndHalfTicks) => [
            -24.0 / 128.0,
            -16.0 / 128.0,
            16.0 / 128.0,
            24.0 / 128.0,
            0.0,
            0.0,
        ],
        (2, _) => {
            let scale = super::unit_draw_scale(oxide_sim::UnitKind::Flakhound) / 128.0;
            [
                -23.0 * scale,
                -9.0 * scale,
                9.0 * scale,
                23.0 * scale,
                0.0,
                0.0,
            ]
        }
        _ => [-0.075, 0.075, 0.0, 0.0, 0.0, 0.0],
    };
    std::array::from_fn(|index| {
        if index >= count * 2 {
            None
        } else {
            groups[usize::from(index >= count)].map(|progress| (offsets[index], progress))
        }
    })
}

fn forge_spot_phases(progress: f32) -> (f32, f32) {
    let progress = progress.clamp(0.0, 1.0);
    let travel = (progress / FORGE_SPOT_TRAVEL_FRACTION).clamp(0.0, 1.0);
    let impact = ((progress - FORGE_SPOT_TRAVEL_FRACTION) / (1.0 - FORGE_SPOT_TRAVEL_FRACTION))
        .clamp(0.0, 1.0);
    (travel, impact)
}

fn shot_impact_progress(style: crate::game::ShotStyle, age: f32) -> f32 {
    use crate::game::ShotStyle;
    match style {
        ShotStyle::Contact | ShotStyle::Rail => (age / style.life()).clamp(0.0, 1.0),
        ShotStyle::ForgeSpot | ShotStyle::Kinetic { .. } => forge_spot_phases(age / style.life()).1,
        ShotStyle::FlakBurst { yoke_delay, .. } => {
            let arrival = yoke_delay.seconds() + FLAK_ROUND_TRAVEL;
            ((age - arrival) / (style.life() - arrival)).clamp(0.0, 1.0)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShotVisibility {
    Hidden,
    ImpactOnly,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SapperEffectVisibility {
    body: bool,
    bloom: bool,
}

fn sapper_effect_visibility(
    unredacted: bool,
    source_visible: bool,
    impact_visible: bool,
) -> SapperEffectVisibility {
    SapperEffectVisibility {
        body: unredacted || source_visible,
        bloom: unredacted || impact_visible,
    }
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

fn draw_splash_bloom(sprites: &Sprites, center: Vec2, zoom: f32, radius: f32, progress: f32) {
    super::destruction::draw_hit(sprites, center, zoom, radius, progress);
}

pub(crate) fn draw_fx(game: &crate::game::Scene<'_>, sprites: &Sprites) {
    let sees = |p: Vec2| {
        game.my_vision()
            .visible(TilePos::new(p.x.floor() as i32, p.y.floor() as i32))
    };
    // Real shells render from sim state, aged by sim ticks: pause holds
    // them mid-air, speed changes track, and a replay loaded mid-flight
    // restores them — no wall-clock effect can drift from the rules.
    let shell_speed = oxide_sim::stats::SHELL_SPEED.to_num::<f32>();
    let now = game.state.current_tick() as f32 + game.presentation.tick_fraction();
    for (index, shell) in game.state.shells().iter().enumerate() {
        if bomber_release(game, index).is_some() {
            continue;
        }
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
        let from = shell_visual_origin(launch, to, shell.shooter, shell.kind);
        // Fog rule: own and allied shells draw throughout their flight;
        // a hostile shell appears only while its current local segment
        // crosses visible ground. Nothing anchors a trail at a fogged
        // muzzle and pinpoints the hidden artillery.
        let mine = !game.state.hostile(game.presentation.human, shell.player);
        let flat_seen = |k: f32| sees(from.lerp(to, k));
        // Reconstruct flight length the way the launch computed it, so
        // the shell lands exactly when the sim resolves the hit.
        let total = (launch.distance(to) / shell_speed).ceil().max(1.0);
        let elapsed = if shell.kind == oxide_sim::ProjectileKind::Missile {
            crate::audio_timeline::missile_elapsed_ticks(now, shell.arrival, total)
        } else {
            total - (shell.arrival as f32 - now)
        };
        let flight_progress = (elapsed / total).clamp(0.0, 1.0);
        let t = if shell.kind == oxide_sim::ProjectileKind::Missile {
            missile_travel_progress(flight_progress, total, from.distance(to))
        } else {
            flight_progress
        };
        if !game.presentation.all_seeing() && !mine && !flat_seen(t) {
            continue;
        }
        let a = game.presentation.camera.to_screen(from);
        let b = game.presentation.camera.to_screen(to);
        let dist = (b - a).length();
        let lift = if shell.kind == oxide_sim::ProjectileKind::Shell {
            shell_arc_lift(dist, game.presentation.camera.zoom, shell.shooter)
        } else {
            0.0
        };
        let bastion_shell = shell.kind == oxide_sim::ProjectileKind::Shell
            && matches!(shell.shooter, oxide_sim::Target::Building(_));
        let artillery_heading = game
            .presentation
            .projectile_releases
            .artillery_heading(shell);
        let at = |t: f32| {
            if let Some(heading) = artillery_heading {
                return game
                    .presentation
                    .camera
                    .to_screen(bombard_shell_position(launch, to, heading, t));
            }
            let flat = a.lerp(b, t);
            if bastion_shell {
                return flat;
            }
            vec2(flat.x, flat.y - lift * 4.0 * t * (1.0 - t))
        };
        let tail_t = if shell.kind == oxide_sim::ProjectileKind::Missile {
            (t - 0.65 / from.distance(to).max(f32::EPSILON)).max(0.0)
        } else {
            shell_tail_start(t, from.distance(to))
        };
        let tail = at(tail_t);
        let shell_at = at(t);
        let flat = a.lerp(b, t);
        if shell.kind != oxide_sim::ProjectileKind::Shell {
            let direction = (at((t + 0.01).min(1.0)) - at((t - 0.01).max(0.0))).normalize_or_zero();
            let normal = vec2(-direction.y, direction.x);
            let missile = shell.kind == oxide_sim::ProjectileKind::Missile;
            let length = game.presentation.camera.zoom * if missile { 0.375 } else { 0.28 };
            let width = game.presentation.camera.zoom * if missile { 0.078125 } else { 0.13 };
            let center = shell_at
                - vec2(
                    0.0,
                    if missile {
                        0.0
                    } else {
                        game.presentation.camera.zoom * 0.18 * (1.0 - t)
                    },
                );
            let back = center - direction * length * 0.5;
            let nose = center + direction * length * 0.5;
            let motor = missile_motor_strength(flight_progress, total);
            if missile
                && motor > 0.0
                && (mine || game.presentation.all_seeing() || flat_seen(tail_t))
            {
                let exhaust = back
                    - direction
                        * game.presentation.camera.zoom
                        * motor
                        * (0.16 + 0.025 * (t * 97.0).sin());
                draw_line(
                    exhaust.x,
                    exhaust.y,
                    back.x,
                    back.y,
                    width * 0.60,
                    Color::from_rgba(182, 83, 35, 180),
                );
                let core = back - direction * game.presentation.camera.zoom * 0.08 * motor;
                draw_line(
                    core.x,
                    core.y,
                    back.x,
                    back.y,
                    width * 0.30,
                    Color::from_rgba(246, 199, 116, 255),
                );
                if (exhaust - tail).dot(direction) > 0.0 {
                    draw_line(
                        tail.x,
                        tail.y,
                        exhaust.x,
                        exhaust.y,
                        width * 0.70,
                        Color::from_rgba(112, 103, 90, 85),
                    );
                }
            }
            draw_line(
                back.x,
                back.y,
                nose.x,
                nose.y,
                width
                    + if missile {
                        game.presentation.camera.zoom * 0.035
                    } else {
                        2.0
                    },
                Color::from_rgba(12, 13, 17, 255),
            );
            let shoulder = nose - direction * length * 0.18;
            draw_line(
                back.x,
                back.y,
                shoulder.x,
                shoulder.y,
                width,
                Color::from_rgba(151, 146, 134, 255),
            );
            draw_triangle(
                nose,
                shoulder + normal * width * 0.5,
                shoulder - normal * width * 0.5,
                Color::from_rgba(210, 199, 171, 255),
            );
            let fin = back + direction * length * 0.16;
            draw_triangle(
                back,
                fin + normal * width,
                fin - normal * width,
                Color::from_rgba(92, 74, 59, 255),
            );
            continue;
        }
        if let Some(direction) =
            artillery_heading.or_else(|| bastion_shell.then(|| (to - launch).normalize_or_zero()))
        {
            let direction = direction.normalize_or_zero();
            let normal = vec2(-direction.y, direction.x);
            let height = 4.0 * t * (1.0 - t);
            let scale = game.presentation.camera.zoom * (1.0 + height * 0.22);
            let width = scale * if bastion_shell { 0.12 } else { 0.14 };
            let length = scale * if bastion_shell { 0.34 } else { 0.30 };
            let back = shell_at - direction * length * 0.5;
            let nose = shell_at + direction * length * 0.5;
            let shoulder = nose - direction * length * 0.22;
            let shadow = shell_at
                + game.presentation.camera.zoom * (vec2(0.04, 0.06) + vec2(0.18, 0.26) * height);
            let shadow_half = direction * game.presentation.camera.zoom * 0.10;
            draw_line(
                (shadow - shadow_half).x,
                (shadow - shadow_half).y,
                (shadow + shadow_half).x,
                (shadow + shadow_half).y,
                game.presentation.camera.zoom * (0.10 + 0.03 * height),
                Color::new(0.02, 0.02, 0.025, 0.34 - 0.16 * height),
            );
            draw_line(
                back.x,
                back.y,
                shoulder.x,
                shoulder.y,
                width + game.presentation.camera.zoom * 0.035,
                Color::from_rgba(14, 15, 18, 255),
            );
            draw_line(
                back.x,
                back.y,
                shoulder.x,
                shoulder.y,
                width,
                Color::from_rgba(123, 128, 127, 255),
            );
            draw_triangle(
                nose,
                shoulder + normal * width * 0.5,
                shoulder - normal * width * 0.5,
                Color::from_rgba(181, 171, 147, 255),
            );
            let band = back + direction * length * 0.16;
            draw_line(
                (band - normal * width * 0.5).x,
                (band - normal * width * 0.5).y,
                (band + normal * width * 0.5).x,
                (band + normal * width * 0.5).y,
                game.presentation.camera.zoom * 0.035,
                Color::from_rgba(149, 107, 58, 255),
            );
            continue;
        }
        let radius = (game.presentation.camera.zoom * 0.075).clamp(2.2, 4.0);
        // The tiny flat-path shadow makes the restrained lift legible
        // without restoring the old launch-to-impact glowing arc.
        draw_circle(
            flat.x,
            flat.y,
            radius * 0.7,
            Color::new(0.03, 0.03, 0.04, 0.35),
        );
        if game.presentation.all_seeing() || mine || flat_seen(tail_t) {
            let before = at((t - 0.01).max(0.0));
            let after = at((t + 0.01).min(1.0));
            let travel = (after - before).normalize_or_zero();
            let travel = if travel.length_squared() <= f32::EPSILON {
                (b - a).normalize_or_zero()
            } else {
                travel
            };
            let normal = vec2(-travel.y, travel.x);
            let offset = normal * radius * 0.34;
            let tail_end = shell_at - travel * radius * 0.78 + offset;
            let tail_start = tail + offset;
            let warm_start = tail_start.lerp(tail_end, 0.50);
            draw_line(
                tail_start.x,
                tail_start.y,
                tail_end.x,
                tail_end.y,
                radius * 0.56,
                Color::new(0.52, 0.18, 0.09, 0.92),
            );
            draw_line(
                warm_start.x,
                warm_start.y,
                tail_end.x,
                tail_end.y,
                radius * 0.28,
                Color::new(0.98, 0.53, 0.18, 0.88),
            );
        }
        let before = at((t - 0.01).max(0.0));
        let after = at((t + 0.01).min(1.0));
        let travel = (after - before).normalize_or_zero();
        let travel = if travel.length_squared() <= f32::EPSILON {
            (b - a).normalize_or_zero()
        } else {
            travel
        };
        draw_circle(
            shell_at.x,
            shell_at.y,
            radius * 1.35,
            Color::new(0.96, 0.42, 0.12, 0.16),
        );
        let body_start = shell_at - travel * radius * 0.80;
        let body_end = shell_at + travel * radius * 0.36;
        draw_line(
            body_start.x,
            body_start.y,
            body_end.x,
            body_end.y,
            radius * 1.28,
            Color::new(0.10, 0.09, 0.09, 1.0),
        );
        let nose = body_end;
        draw_circle(
            nose.x,
            nose.y,
            radius * 0.54,
            Color::new(1.0, 0.82, 0.48, 1.0),
        );
    }
    for fx in &game.presentation.fx {
        // A visible impact may always spark so incoming damage reads.
        // Directional geometry still requires a visible source and must
        // not pinpoint a fogged shooter.
        let in_sight = match fx.kind {
            EffectKind::DirectShot {
                style, from, to, ..
            } => shot_visibility(style, sees(from), sees(to)) != ShotVisibility::Hidden,
            EffectKind::SapperDetonation {
                at,
                blast_at,
                player,
                source_witnessed,
                impact_witnessed,
                ..
            } => {
                let visibility = sapper_effect_visibility(
                    player == game.presentation.human,
                    source_witnessed || sees(at),
                    impact_witnessed || sees(blast_at),
                );
                visibility.body || visibility.bloom
            }
            EffectKind::Collapse { at, .. } | EffectKind::Impact { at, .. } => sees(at),
            EffectKind::Puff { at } => sees(at),
            // Falling fragments and airframes apply fog at their moving positions.
            EffectKind::Falling { .. } => true,
            EffectKind::Burst { at, .. } => sees(at),
            EffectKind::Debris { at, .. } => sees(at),
            // Own-order acknowledgments always show; fogged targets are
            // already impossible to order onto.
            EffectKind::Ping { .. } => true,
        };
        if !game.presentation.all_seeing() && !in_sight {
            continue;
        }
        match fx.kind {
            EffectKind::DirectShot {
                style,
                from,
                to,
                splash,
                ..
            } => {
                use crate::game::ShotStyle;
                let a = game.presentation.camera.to_screen(from);
                let b = game.presentation.camera.to_screen(to);
                let age = fx.age_at(game.state.current_tick(), game.presentation.tick_fraction());
                let progress = (age / style.life()).clamp(0.0, 1.0);
                let fade = 1.0 - progress;
                let impact = shot_impact_progress(style, age);
                let visibility = shot_visibility(
                    style,
                    game.presentation.all_seeing() || sees(from),
                    game.presentation.all_seeing() || sees(to),
                );
                if visibility == ShotVisibility::ImpactOnly {
                    if let Some(radius) = splash
                        && impact > 0.0
                    {
                        draw_splash_bloom(
                            sprites,
                            b,
                            game.presentation.camera.zoom,
                            radius,
                            impact,
                        );
                    }
                    let seed = (to.x * 31.7 + to.y * 17.3).abs();
                    for i in 0..3 {
                        let angle = seed + i as f32 * 2.1;
                        let reach = game.presentation.camera.zoom * (0.08 + impact * 0.12);
                        let tip = b + vec2(angle.cos(), angle.sin()) * reach;
                        draw_line(
                            b.x,
                            b.y,
                            tip.x,
                            tip.y,
                            1.5,
                            Color::new(0.95, 0.67, 0.34, 1.0 - impact),
                        );
                    }
                    continue;
                }
                if let Some(radius) = splash
                    && impact > 0.0
                {
                    // The area bloom is part of the round-arrival phase,
                    // behind the projectiles, rather than an explosion the
                    // rounds visibly fly into.
                    draw_splash_bloom(sprites, b, game.presentation.camera.zoom, radius, impact);
                }
                match style {
                    ShotStyle::Contact => {}
                    ShotStyle::Kinetic { heavy } => {
                        let (travel, impact) = forge_spot_phases(progress);
                        let direction = (b - a).normalize_or_zero();
                        let zoom = game.presentation.camera.zoom;
                        let round = a.lerp(b, travel);
                        let length = zoom * if heavy { 0.19 } else { 0.09 };
                        let tail = round - direction * length.min(round.distance(a));
                        let alpha = 1.0 - impact;
                        draw_line(
                            tail.x,
                            tail.y,
                            round.x,
                            round.y,
                            (zoom * if heavy { 0.065 } else { 0.03 }).max(1.0),
                            Color::new(0.91, 0.79, 0.57, alpha),
                        );
                        if impact > 0.0 {
                            let normal = vec2(-direction.y, direction.x);
                            for side in [-1.0, 0.0, 1.0] {
                                let spread = (-direction + normal * side * 1.4).normalize_or_zero();
                                let reach =
                                    zoom * (0.05 + impact * if heavy { 0.25 } else { 0.16 });
                                let start = b + spread * reach * 0.55;
                                let end = b + spread * reach;
                                draw_line(
                                    start.x,
                                    start.y,
                                    end.x,
                                    end.y,
                                    1.0,
                                    Color::new(0.84, 0.66, 0.41, alpha),
                                );
                            }
                        }
                    }
                    ShotStyle::ForgeSpot => {
                        let (travel, impact) = forge_spot_phases(progress);
                        let round = a.lerp(b, travel);
                        let round_alpha = 1.0 - impact;
                        draw_circle(
                            round.x,
                            round.y,
                            3.8,
                            Color::new(1.0, 0.38, 0.10, 0.20 * round_alpha),
                        );
                        draw_circle(
                            round.x,
                            round.y,
                            2.3,
                            Color::new(0.42, 0.19, 0.09, round_alpha),
                        );
                        draw_circle(
                            round.x,
                            round.y,
                            1.35,
                            Color::new(1.0, 0.85, 0.52, round_alpha),
                        );
                        if impact > 0.0 {
                            let radius = game.presentation.camera.zoom * (0.05 + impact * 0.18);
                            draw_circle_lines(
                                b.x,
                                b.y,
                                radius,
                                1.5,
                                Color::new(1.0, 0.62, 0.22, 1.0 - impact),
                            );
                        }
                    }
                    ShotStyle::Rail => {
                        draw_line(
                            a.x,
                            a.y,
                            b.x,
                            b.y,
                            game.presentation.camera.zoom * 0.10 * fade.max(0.25),
                            Color::new(0.70, 0.76, 0.80, 0.18 * fade * fade),
                        );
                        draw_line(
                            a.x,
                            a.y,
                            b.x,
                            b.y,
                            (game.presentation.camera.zoom * 0.038).max(0.8),
                            Color::new(0.89, 0.89, 0.80, fade * fade),
                        );
                    }
                    ShotStyle::FlakBurst {
                        yoke_delay,
                        rounds_per_yoke,
                    } => {
                        let direction = (b - a).normalize_or_zero();
                        let normal = vec2(-direction.y, direction.x);
                        for (barrel, round) in flak_barrel_rounds(age, yoke_delay, rounds_per_yoke)
                            .into_iter()
                            .flatten()
                        {
                            let offset = normal * barrel * game.presentation.camera.zoom;
                            let end = b + offset;
                            let at = (a + offset).lerp(end, round);
                            draw_circle(at.x, at.y, 3.4, Color::new(0.98, 0.43, 0.12, 0.18));
                            draw_circle(at.x, at.y, 2.0, Color::new(0.33, 0.24, 0.13, 1.0));
                            draw_circle(at.x, at.y, 1.2, Color::new(1.0, 0.84, 0.48, 1.0));
                        }
                        let seed = (to.x * 31.7 + to.y * 17.3).abs();
                        for i in 0..3 {
                            let angle = seed + i as f32 * 2.1;
                            let reach = impact * game.presentation.camera.zoom * 0.28;
                            let puff = b + vec2(angle.cos(), angle.sin()) * reach;
                            draw_circle(
                                puff.x,
                                puff.y,
                                game.presentation.camera.zoom * 0.07 * (1.0 - progress * 0.45),
                                Color::new(0.66, 0.65, 0.58, 0.42 * impact * fade),
                            );
                        }
                    }
                }
            }
            EffectKind::SapperDetonation {
                at,
                blast_at,
                rotation,
                player,
                faction,
                source_witnessed,
                impact_witnessed,
                ..
            } => {
                let age = fx.age_at(game.state.current_tick(), game.presentation.tick_fraction());
                let progress = (age / (crate::game::TICK_DT * 2.0)).clamp(0.0, 1.0);
                let fade = 1.0 - progress;
                let body = game.presentation.camera.to_screen(at);
                let size = game.presentation.camera.zoom
                    * super::unit_draw_scale(oxide_sim::UnitKind::Sapper);
                let body_size = vec2(size, size);
                let visibility = sapper_effect_visibility(
                    game.presentation.all_seeing() || player == game.presentation.human,
                    source_witnessed || sees(at),
                    impact_witnessed || sees(blast_at),
                );
                if visibility.body {
                    sprites.draw(
                        body.x - size * 0.5,
                        body.y - size * 0.5,
                        Color::new(1.0, 1.0, 1.0, fade),
                        DrawTextureParams {
                            dest_size: Some(body_size),
                            source: Some(sprites.unit_action(
                                oxide_sim::UnitKind::Sapper,
                                faction,
                                2,
                            )),
                            rotation,
                            ..Default::default()
                        },
                    );
                    if let Some(mut tint) = seat_identity_tint(game, player) {
                        tint.a *= fade;
                        sprites.draw(
                            body.x - size * 0.5,
                            body.y - size * 0.5,
                            tint,
                            DrawTextureParams {
                                dest_size: Some(body_size),
                                source: Some(
                                    sprites.unit_action_accent(oxide_sim::UnitKind::Sapper, 2),
                                ),
                                rotation,
                                ..Default::default()
                            },
                        );
                    }
                }
                if visibility.bloom {
                    draw_splash_bloom(
                        sprites,
                        game.presentation.camera.to_screen(blast_at),
                        game.presentation.camera.zoom,
                        oxide_sim::stats::SAPPER_BLAST_RADIUS.to_num::<f32>(),
                        progress,
                    );
                }
            }
            EffectKind::Falling {
                at,
                body,
                seed,
                crash,
                ..
            } => {
                super::destruction::draw_falling(
                    game,
                    sprites,
                    at,
                    body,
                    seed,
                    fx.age_at(game.state.current_tick(), game.presentation.tick_fraction()),
                    crash,
                );
            }
            EffectKind::Collapse { .. } => {}
            EffectKind::Impact {
                at,
                radius,
                payload,
            } => {
                super::destruction::draw_impact(
                    game.presentation.camera.to_screen(at),
                    game.presentation.camera.zoom,
                    radius,
                    fx.age,
                    payload,
                );
            }
            EffectKind::Puff { at } => {
                super::destruction::draw_impact(
                    game.presentation.camera.to_screen(at),
                    game.presentation.camera.zoom,
                    0.45,
                    fx.age,
                    oxide_sim::ProjectileKind::Shell,
                );
            }
            EffectKind::Burst { at, radius } => {
                let center = game.presentation.camera.to_screen(at);
                let progress = (fx.age / 0.35).clamp(0.0, 1.0);
                draw_splash_bloom(
                    sprites,
                    center,
                    game.presentation.camera.zoom,
                    radius,
                    progress,
                );
            }
            EffectKind::Debris { .. } => {}
            EffectKind::Ping { .. } => {} // drawn above the fog, in draw_pings
        }
    }
}

/// Radar blips, drawn above the fog: contacts without identity from the
/// Array's outer ring — the player's own intel, like pings.
pub(crate) fn draw_blips(game: &crate::game::Scene<'_>) {
    if game.presentation.overlay {
        return; // the omniscient overlay already shows the real machines
    }
    let zoom = game.presentation.camera.zoom;
    for &tile in game.my_vision().contacts() {
        let center = game
            .presentation
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
    AirWeapon,
    DeadZone,
    Vision,
    Radar,
    Repair,
    EconomySupport,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum BuildingRangeShape {
    Circle { center: Vec2, radius: f32 },
    FootprintOffset { min: Vec2, max: Vec2, radius: f32 },
    FootprintSquare { min: Vec2, max: Vec2, radius: f32 },
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
            Self::FootprintSquare { min, max, radius } => {
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
    fn pattern(self) -> &'static [f32] {
        match self {
            Self::Solid => &[],
            Self::ShortDash => &[6.0, 6.0],
            Self::LongDash => &[12.0, 8.0],
            Self::Dotted => &[2.5, 6.5],
            Self::DashDot => &[11.0, 5.0, 3.0, 7.0],
        }
    }
}

fn range_stroke(kind: BuildingRangeKind) -> RangeStroke {
    match kind {
        BuildingRangeKind::Weapon | BuildingRangeKind::AirWeapon => RangeStroke::Solid,
        BuildingRangeKind::DeadZone => RangeStroke::ShortDash,
        BuildingRangeKind::Vision => RangeStroke::LongDash,
        BuildingRangeKind::Radar => RangeStroke::Dotted,
        BuildingRangeKind::Repair => RangeStroke::DashDot,
        BuildingRangeKind::EconomySupport => RangeStroke::Solid,
    }
}

fn range_icon(kind: BuildingRangeKind) -> crate::panel::CapabilityIcon {
    use crate::panel::CapabilityIcon;
    match kind {
        BuildingRangeKind::Weapon => CapabilityIcon::Weapon,
        BuildingRangeKind::AirWeapon => CapabilityIcon::AirWeapon,
        BuildingRangeKind::DeadZone => CapabilityIcon::DeadZone,
        BuildingRangeKind::Vision => CapabilityIcon::Vision,
        BuildingRangeKind::Radar => CapabilityIcon::Radar,
        BuildingRangeKind::Repair => CapabilityIcon::Repair,
        BuildingRangeKind::EconomySupport => CapabilityIcon::EconomySupport,
    }
}

fn weapon_range_kind(weapon: &oxide_sim::stats::WeaponStats) -> BuildingRangeKind {
    match crate::panel::weapon_capability_icon(weapon) {
        crate::panel::CapabilityIcon::AirWeapon => BuildingRangeKind::AirWeapon,
        _ => BuildingRangeKind::Weapon,
    }
}

fn dead_zone_fill(color: Color) -> Color {
    Color::new(color.r, color.g, color.b, 0.035)
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
    occluders: &[(Vec2, Vec2)],
) {
    let mut clipper = RangeClipper::new(points, occluders);
    visit_stroke_segments(points, stroke, scale, |a, b| {
        clipper.visit_visible(a, b, |from, to| {
            draw_line(from.x, from.y, to.x, to.y, thickness, color);
        });
    });
}

fn visit_stroke_segments(
    points: &[Vec2],
    stroke: RangeStroke,
    scale: f32,
    mut visit: impl FnMut(Vec2, Vec2),
) {
    let pattern = stroke.pattern();
    let scale = scale.max(0.25);
    let mut index = 0;
    let mut remaining = pattern
        .first()
        .map_or(f32::INFINITY, |length| length * scale);
    for pair in points.windows(2) {
        let delta = pair[1] - pair[0];
        let length = delta.length();
        if length <= f32::EPSILON {
            continue;
        }
        let direction = delta / length;
        let mut offset = 0.0;
        while offset < length {
            let end = (offset + remaining).min(length);
            if index % 2 == 0 && end > offset {
                visit(pair[0] + direction * offset, pair[0] + direction * end);
            }
            remaining = if end == offset {
                0.0
            } else {
                remaining - (end - offset)
            };
            offset = end;
            if remaining <= f32::EPSILON && !pattern.is_empty() {
                index = (index + 1) % pattern.len();
                remaining = pattern[index] * scale;
            }
        }
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

fn square_footprint_path(min: Vec2, max: Vec2, radius: f32) -> [Vec2; 5] {
    let outer_min = min - vec2(radius, radius);
    let outer_max = max + vec2(radius, radius);
    [
        outer_min,
        vec2(outer_max.x, outer_min.y),
        outer_max,
        vec2(outer_min.x, outer_max.y),
        outer_min,
    ]
}

fn footprint_distance_aura(distance: i32) -> f32 {
    // Buildings draw to the outside edge of their occupied tiles, while the
    // simulation compares the occupied tile coordinates themselves. Two
    // nearest tiles eight coordinates apart have seven empty tile-widths
    // between their footprint edges.
    distance.saturating_sub(1) as f32
}

/// One range vocabulary serves selected buildings and placement ghosts.
/// Keeping the visit pure makes omissions testable without a graphics
/// context and avoids allocating in the frame loop.
fn visit_building_ranges(
    anchor: Vec2,
    kind: oxide_sim::BuildingKind,
    tier: u8,
    mut visit: impl FnMut(BuildingRange),
) {
    let stats = kind.tier_stats(tier);
    let size = vec2(stats.size.0 as f32, stats.size.1 as f32);
    let center = anchor + size * 0.5;
    if let Some(weapon) = stats.weapons.first() {
        visit(BuildingRange {
            kind: weapon_range_kind(weapon),
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
    if matches!(
        kind,
        oxide_sim::BuildingKind::Foundry | oxide_sim::BuildingKind::Extractor
    ) {
        visit(BuildingRange {
            kind: BuildingRangeKind::EconomySupport,
            shape: BuildingRangeShape::FootprintSquare {
                min: anchor,
                max: anchor + size,
                radius: footprint_distance_aura(oxide_sim::stats::EXTRACTOR_SUPPORT_RADIUS),
            },
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EconomySupportLink {
    extractor: oxide_sim::BuildingId,
    foundry: oxide_sim::BuildingId,
}

fn selected_economy_support_links(game: &crate::game::Scene<'_>) -> Vec<EconomySupportLink> {
    let Some(oxide_sim::Target::Building(selected)) = range_subject(
        &game.presentation.selection.units,
        &game.presentation.selection.buildings,
    ) else {
        return Vec::new();
    };
    let Some(building) = game.state.building(selected) else {
        return Vec::new();
    };
    if building.player != game.presentation.human || !building.built || building.hp == 0 {
        return Vec::new();
    }

    match building.kind {
        oxide_sim::BuildingKind::Extractor => game
            .state
            .buildings()
            .iter()
            .filter(|candidate| candidate.kind == oxide_sim::BuildingKind::Foundry)
            .filter(|candidate| game.state.extractor_supported_by(building.id, candidate.id))
            .map(|candidate| EconomySupportLink {
                extractor: building.id,
                foundry: candidate.id,
            })
            .collect(),
        oxide_sim::BuildingKind::Foundry => game
            .state
            .buildings()
            .iter()
            .filter(|candidate| candidate.kind == oxide_sim::BuildingKind::Extractor)
            .filter(|candidate| game.state.extractor_supported_by(candidate.id, building.id))
            .map(|candidate| EconomySupportLink {
                extractor: candidate.id,
                foundry: building.id,
            })
            .collect(),
        _ => Vec::new(),
    }
}

const ECONOMY_SUPPORT_COLOR: Color = Color::new(0.72, 0.63, 0.46, 0.58);

fn building_screen_bounds(
    game: &crate::game::Scene<'_>,
    building: &oxide_sim::Building,
) -> (Vec2, Vec2) {
    let (width, height) = building.stats().size;
    let min = game
        .presentation
        .camera
        .to_screen(vec2(building.anchor.x as f32, building.anchor.y as f32));
    (
        min,
        min + vec2(width as f32, height as f32) * game.presentation.camera.zoom,
    )
}

fn footprint_link(a: (Vec2, Vec2), b: (Vec2, Vec2), padding: f32) -> Option<[Vec2; 2]> {
    let a_center = (a.0 + a.1) * 0.5;
    let b_center = (b.0 + b.1) * 0.5;
    let delta = b_center - a_center;
    let distance = delta.length();
    if distance <= f32::EPSILON {
        return None;
    }
    let direction = delta / distance;
    let exit = |bounds: (Vec2, Vec2)| {
        let half = (bounds.1 - bounds.0) * 0.5 + Vec2::splat(padding);
        (half.x / direction.x.abs()).min(half.y / direction.y.abs())
    };
    let a_exit = exit(a);
    let b_exit = exit(b);
    (a_exit + b_exit < distance)
        .then_some([a_center + direction * a_exit, b_center - direction * b_exit])
}

fn line_footprint_interval(a: Vec2, b: Vec2, min: Vec2, max: Vec2) -> Option<(f32, f32)> {
    let delta = b - a;
    let mut enter: f32 = 0.0;
    let mut exit: f32 = 1.0;
    for (origin, direction, lo, hi) in [(a.x, delta.x, min.x, max.x), (a.y, delta.y, min.y, max.y)]
    {
        if direction.abs() <= f32::EPSILON {
            if origin < lo || origin > hi {
                return None;
            }
        } else {
            let first = (lo - origin) / direction;
            let last = (hi - origin) / direction;
            enter = enter.max(first.min(last));
            exit = exit.min(first.max(last));
        }
    }
    (enter < exit).then_some((enter, exit))
}

fn range_occluders(game: &crate::game::Scene<'_>) -> Vec<(Vec2, Vec2)> {
    let margin = Vec2::splat(2.0 * ui_scale());
    let viewport = (-margin, game.presentation.camera.viewport() + margin);
    game.state
        .buildings()
        .iter()
        .filter(|building| {
            building.player == game.presentation.human
                || game.presentation.all_seeing()
                || (building.tiles().any(|tile| game.my_vision().visible(tile))
                    && game
                        .state
                        .building_apparent(game.presentation.human, building))
        })
        .map(|building| {
            let (min, max) = building_screen_bounds(game, building);
            (min - margin, max + margin)
        })
        .filter(|&bounds| bounds_overlap(bounds, viewport))
        .collect()
}

fn bounds_overlap(a: (Vec2, Vec2), b: (Vec2, Vec2)) -> bool {
    a.0.x <= b.1.x && a.1.x >= b.0.x && a.0.y <= b.1.y && a.1.y >= b.0.y
}

struct RangeClipper {
    occluders: Vec<(Vec2, Vec2)>,
    intervals: Vec<(f32, f32)>,
}

impl RangeClipper {
    fn new(points: &[Vec2], occluders: &[(Vec2, Vec2)]) -> Self {
        let bounds = points.iter().fold(
            (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY)),
            |(min, max), &point| (min.min(point), max.max(point)),
        );
        let occluders: Vec<_> = occluders
            .iter()
            .copied()
            .filter(|&occluder| bounds_overlap(occluder, bounds))
            .collect();
        let intervals = Vec::with_capacity(occluders.len());
        Self {
            occluders,
            intervals,
        }
    }

    fn visit_visible(&mut self, a: Vec2, b: Vec2, mut visit: impl FnMut(Vec2, Vec2)) {
        self.intervals.clear();
        self.intervals.extend(
            self.occluders
                .iter()
                .filter_map(|&(min, max)| line_footprint_interval(a, b, min, max)),
        );
        self.intervals.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        let mut cursor: f32 = 0.0;
        for &(start, end) in self.intervals.iter().chain(std::iter::once(&(1.0, 1.0))) {
            if cursor < start {
                visit(a.lerp(b, cursor), a.lerp(b, start));
            }
            cursor = cursor.max(end);
        }
    }
}

fn visit_active_building_ranges(
    game: &crate::game::Scene<'_>,
    input: &InputState,
    mut visit: impl FnMut(BuildingRange),
) {
    if let Some(oxide_sim::Target::Building(id)) = range_subject(
        &game.presentation.selection.units,
        &game.presentation.selection.buildings,
    ) && let Some(building) = game.state.building(id)
    {
        visit_building_ranges(
            vec2(building.anchor.x as f32, building.anchor.y as f32),
            building.kind,
            building.tier,
            &mut visit,
        );
    }
    if let Some(kind) = input.placing {
        let world = game.presentation.camera.to_world(input.mouse);
        let clicked = TilePos::new(world.x.floor() as i32, world.y.floor() as i32);
        let anchor = crate::input::placement_anchor(game, kind, clicked);
        visit_building_ranges(vec2(anchor.x as f32, anchor.y as f32), kind, 0, visit);
    }
}

fn draw_economy_ground(game: &crate::game::Scene<'_>, shape: BuildingRangeShape) {
    let scale = ui_scale();
    if let BuildingRangeShape::FootprintSquare { min, max, radius } = shape {
        let min = game
            .presentation
            .camera
            .to_screen(min - Vec2::splat(radius));
        let max = game
            .presentation
            .camera
            .to_screen(max + Vec2::splat(radius));
        let depth = (10.0 * scale).min((max - min).min_element() * 0.25);
        let steps = depth.ceil() as usize;
        for step in 0..steps {
            let inset = step as f32 * depth / steps as f32;
            let thickness = depth / steps as f32;
            let lo = min + Vec2::splat(inset);
            let hi = max - Vec2::splat(inset);
            let color = Color::new(
                ECONOMY_SUPPORT_COLOR.r,
                ECONOMY_SUPPORT_COLOR.g,
                ECONOMY_SUPPORT_COLOR.b,
                0.12 * (1.0 - inset / depth).powi(2),
            );
            draw_rectangle(lo.x, lo.y, hi.x - lo.x, thickness, color);
            draw_rectangle(lo.x, hi.y - thickness, hi.x - lo.x, thickness, color);
            draw_rectangle(
                lo.x,
                lo.y + thickness,
                thickness,
                hi.y - lo.y - 2.0 * thickness,
                color,
            );
            draw_rectangle(
                hi.x - thickness,
                lo.y + thickness,
                thickness,
                hi.y - lo.y - 2.0 * thickness,
                color,
            );
        }
    }
}

fn draw_economy_support_links(
    game: &crate::game::Scene<'_>,
    scale: f32,
    color: Color,
    occluders: &[(Vec2, Vec2)],
) {
    let links = selected_economy_support_links(game);
    for link in &links {
        let Some(extractor) = game.state.building(link.extractor) else {
            continue;
        };
        let Some(foundry) = game.state.building(link.foundry) else {
            continue;
        };
        let extractor = building_screen_bounds(game, extractor);
        let foundry = building_screen_bounds(game, foundry);
        if let Some([a, b]) = footprint_link(extractor, foundry, 3.0 * scale) {
            let mut clipper = RangeClipper::new(&[a, b], occluders);
            clipper.visit_visible(a, b, |from, to| {
                draw_line(from.x, from.y, to.x, to.y, 1.2 * scale, color);
            });
        }
    }
    let mut endpoints: Vec<_> = links
        .iter()
        .flat_map(|link| [link.extractor, link.foundry])
        .collect();
    endpoints.sort_unstable();
    endpoints.dedup();
    let footprints = endpoints
        .into_iter()
        .filter(|id| !game.presentation.selection.buildings.contains(id))
        .filter_map(|id| game.state.building(id))
        .map(|building| (building.anchor, building.stats().size));
    for bracket in support_brackets::junctions(footprints, game.presentation.camera.zoom, scale) {
        let origin = game
            .presentation
            .camera
            .to_screen(vec2(bracket.corner.x as f32, bracket.corner.y as f32));
        for [from, to] in bracket.segments(origin) {
            draw_line(from.x, from.y, to.x, to.y, scale, color);
        }
    }
}

#[derive(Clone, Copy)]
struct RangeIndicator {
    range: BuildingRange,
    icon: crate::panel::CapabilityIcon,
    color: Color,
}

fn range_color(kind: BuildingRangeKind) -> Color {
    match kind {
        BuildingRangeKind::Weapon => Color::new(0.76, 0.46, 0.39, 0.58),
        BuildingRangeKind::AirWeapon => Color::new(0.84, 0.39, 0.53, 0.62),
        BuildingRangeKind::DeadZone => Color::new(0.78, 0.63, 0.40, 0.64),
        BuildingRangeKind::Vision => Color::new(0.60, 0.66, 0.73, 0.48),
        BuildingRangeKind::Radar => Color::new(0.43, 0.67, 0.63, 0.58),
        BuildingRangeKind::Repair => Color::new(0.55, 0.69, 0.49, 0.58),
        BuildingRangeKind::EconomySupport => ECONOMY_SUPPORT_COLOR,
    }
}

fn visit_active_ranges(
    game: &crate::game::Scene<'_>,
    input: &InputState,
    mut visit: impl FnMut(RangeIndicator),
) {
    visit_active_building_ranges(game, input, |range| {
        visit(RangeIndicator {
            range,
            icon: range_icon(range.kind),
            color: range_color(range.kind),
        });
    });
    if let Some(oxide_sim::Target::Unit(id)) = range_subject(
        &game.presentation.selection.units,
        &game.presentation.selection.buildings,
    ) && let Some(unit) = game.state.unit(id)
        && unit.player == game.presentation.human
    {
        let center = vec2(unit.pos.x.to_num::<f32>(), unit.pos.y.to_num::<f32>());
        let stats = unit.kind.stats();
        for weapon in stats.weapons {
            let kind = weapon_range_kind(weapon);
            visit(RangeIndicator {
                range: BuildingRange {
                    kind,
                    shape: BuildingRangeShape::Circle {
                        center,
                        radius: weapon.range.to_num::<f32>(),
                    },
                },
                icon: range_icon(kind),
                color: range_color(kind),
            });
        }
        if stats
            .weapons
            .iter()
            .any(|weapon| weapon.range.to_num::<f32>() > stats.vision as f32)
        {
            let kind = BuildingRangeKind::Vision;
            visit(RangeIndicator {
                range: BuildingRange {
                    kind,
                    shape: BuildingRangeShape::Circle {
                        center,
                        radius: stats.vision as f32,
                    },
                },
                icon: range_icon(kind),
                color: range_color(kind),
            });
        }
    }
}

fn screen_range_shape(
    game: &crate::game::Scene<'_>,
    shape: BuildingRangeShape,
) -> BuildingRangeShape {
    match shape {
        BuildingRangeShape::Circle { center, radius } => BuildingRangeShape::Circle {
            center: game.presentation.camera.to_screen(center),
            radius: radius * game.presentation.camera.zoom,
        },
        BuildingRangeShape::FootprintOffset { min, max, radius } => {
            BuildingRangeShape::FootprintOffset {
                min: game.presentation.camera.to_screen(min),
                max: game.presentation.camera.to_screen(max),
                radius: radius * game.presentation.camera.zoom,
            }
        }
        BuildingRangeShape::FootprintSquare { min, max, radius } => {
            BuildingRangeShape::FootprintSquare {
                min: game.presentation.camera.to_screen(min),
                max: game.presentation.camera.to_screen(max),
                radius: radius * game.presentation.camera.zoom,
            }
        }
    }
}

fn range_path(shape: BuildingRangeShape, inset: f32) -> Vec<Vec2> {
    match shape {
        BuildingRangeShape::Circle { center, radius } => {
            let mut path = circle_path(center, radius);
            let factor = (radius - inset).max(0.0) / radius.max(f32::EPSILON);
            for point in &mut path {
                *point = center + (*point - center) * factor;
            }
            path
        }
        BuildingRangeShape::FootprintOffset { min, max, radius } => {
            rounded_footprint_path(min, max, (radius - inset).max(0.0))
        }
        BuildingRangeShape::FootprintSquare { min, max, radius } => {
            square_footprint_path(min, max, radius - inset).to_vec()
        }
    }
}

fn range_fade_mesh(shape: BuildingRangeShape, width: f32, color: Color) -> Mesh {
    let radius = match shape {
        BuildingRangeShape::Circle { radius, .. }
        | BuildingRangeShape::FootprintOffset { radius, .. }
        | BuildingRangeShape::FootprintSquare { radius, .. } => radius,
    };
    let width = width.min(radius * 0.5).max(0.0);
    let mut mesh = Mesh {
        vertices: Vec::new(),
        indices: Vec::new(),
        texture: None,
    };
    for (inset, alpha) in [(0.0, 0.12), (width * 0.5, 0.03), (width, 0.0)] {
        for point in range_path(shape, inset) {
            mesh.vertices.push(Vertex::new(
                point.x,
                point.y,
                0.0,
                0.0,
                0.0,
                Color::new(color.r, color.g, color.b, alpha),
            ));
        }
    }
    let count = mesh.vertices.len() / 3;
    for band in 0..2 {
        for point in 0..count - 1 {
            let a = (band * count + point) as u16;
            let b = a + count as u16;
            mesh.indices
                .extend_from_slice(&[a, a + 1, b + 1, a, b + 1, b]);
        }
    }
    mesh
}

pub(crate) fn draw_range_ground(game: &crate::game::Scene<'_>, input: &InputState) {
    visit_active_ranges(game, input, |indicator| {
        if indicator.range.kind == BuildingRangeKind::EconomySupport {
            draw_economy_ground(game, indicator.range.shape);
            return;
        }
        let shape = screen_range_shape(game, indicator.range.shape);
        if indicator.range.kind == BuildingRangeKind::DeadZone
            && let BuildingRangeShape::Circle { center, radius } = shape
        {
            draw_circle(center.x, center.y, radius, dead_zone_fill(indicator.color));
        }
        draw_mesh(&range_fade_mesh(shape, 10.0 * ui_scale(), indicator.color));
    });
}

pub(crate) fn draw_range_rings(game: &crate::game::Scene<'_>, input: &InputState) {
    if range_subject(
        &game.presentation.selection.units,
        &game.presentation.selection.buildings,
    )
    .is_none()
        && input.placing.is_none()
    {
        return;
    }
    let scale = ui_scale();
    let occluders = range_occluders(game);
    draw_economy_support_links(game, scale, ECONOMY_SUPPORT_COLOR, &occluders);
    visit_active_ranges(game, input, |indicator| {
        let shape = screen_range_shape(game, indicator.range.shape);
        let path = range_path(shape, 0.0);
        stroke_patterned_path(
            &path,
            range_stroke(indicator.range.kind),
            scale,
            indicator.color,
            scale,
            &occluders,
        );
        let glyph = match shape {
            BuildingRangeShape::Circle { center, radius } => {
                center + vec2(radius * 0.707, -radius * 0.707)
            }
            BuildingRangeShape::FootprintOffset { min, max, radius } => {
                vec2(max.x + radius * 0.707, min.y - radius * 0.707)
            }
            BuildingRangeShape::FootprintSquare { min, max, radius } => {
                vec2(max.x + radius, min.y - radius)
            }
        };
        let icon_radius = if indicator.icon == crate::panel::CapabilityIcon::AirWeapon {
            7.0
        } else {
            5.6
        };
        draw_capability_icon(
            glyph,
            icon_radius * scale,
            indicator.icon,
            indicator.color,
            true,
        );
    });
}

pub(crate) fn draw_pings(game: &crate::game::Scene<'_>) {
    for fx in &game.presentation.fx {
        let EffectKind::Ping { at, kind } = fx.kind else {
            continue;
        };
        let center = game.presentation.camera.to_screen(at);
        let progress = (fx.age / 0.5).clamp(0.0, 1.0);
        // Damped: a still ring instead of a collapsing one — the verb
        // color still says what was ordered.
        let radius = if reduced_motion() {
            game.presentation.camera.zoom * 0.4
        } else {
            game.presentation.camera.zoom * (0.65 * (1.0 - progress) + 0.12)
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
pub(crate) fn draw_rally_marker(game: &crate::game::Scene<'_>) {
    // A selected producer draws the line to its rally, not just the
    // flag — where fresh machines will walk should read at a glance.
    // OWN producers only, like the flag below: the foreign panel hides
    // rally and orders on purpose, and an inspected enemy building
    // must not leak its intent through this line either.
    for (building, rally) in game
        .presentation
        .selection
        .buildings
        .iter()
        .filter_map(|id| game.state.building(*id))
        .filter(|building| building.player == game.presentation.human)
        .filter_map(|building| building.rally.map(|rally| (building, rally)))
    {
        let a = game.presentation.camera.to_screen(vec2(
            building.anchor.x as f32 + building.stats().size.0 as f32 * 0.5,
            building.anchor.y as f32 + building.stats().size.1 as f32 * 0.5,
        ));
        let b = game
            .presentation
            .camera
            .to_screen(vec2(rally.x as f32 + 0.5, rally.y as f32 + 0.5));
        draw_line(a.x, a.y, b.x, b.y, 1.5, Color::new(0.91, 0.89, 0.85, 0.35));
    }
    for rally in game
        .presentation
        .selection
        .buildings
        .iter()
        .filter_map(|id| game.state.building(*id))
        .filter(|building| building.player == game.presentation.human)
        .filter_map(|building| building.rally)
    {
        draw_rally_flag(game, rally, game.presentation.camera.zoom);
    }
}

pub(crate) fn draw_drag_rect(game: &crate::game::Scene<'_>, input: &InputState) {
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
    let a = game.presentation.camera.to_world(lo);
    let b = game.presentation.camera.to_world(lo + size);
    for unit in game.state.units() {
        if unit.player != game.presentation.human {
            continue;
        }
        let p = vec2(unit.pos.x.to_num::<f32>(), unit.pos.y.to_num::<f32>());
        if p.x >= a.x && p.x <= b.x && p.y >= a.y && p.y <= b.y {
            let screen = game.presentation.camera.to_screen(p);
            draw_circle_lines(
                screen.x,
                screen.y,
                unit.kind.stats().radius.to_num::<f32>() * game.presentation.camera.zoom + 3.0,
                1.5,
                BONE_FAINT,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Game;

    fn economy_support_game() -> Game {
        let mut scenario = oxide_sim::Scenario::skirmish();
        let frames: Vec<_> = scenario
            .map
            .iter()
            .enumerate()
            .flat_map(|(y, row)| {
                row.char_indices()
                    .filter(|(_, tile)| *tile == 'E')
                    .map(move |(x, _)| (x as i32, y as i32))
            })
            .collect();
        assert!(frames.len() >= 3, "fixture needs home and remote frames");
        let last = frames.len() - 1;
        scenario
            .buildings
            .extend(frames.into_iter().enumerate().map(|(index, (x, y))| {
                oxide_sim::scenario::BuildingSpec {
                    player: u8::from(index == last),
                    kind: oxide_sim::BuildingKind::Extractor,
                    x,
                    y,
                }
            }));
        Game::with_viewport(scenario, vec2(1280.0, 800.0)).expect("support fixture builds")
    }

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
    fn support_links_exist_only_for_connected_own_endpoints() {
        let mut game = economy_support_game();
        let supported = game
            .state
            .buildings()
            .iter()
            .find(|building| {
                building.player == game.presentation.human
                    && building.kind == oxide_sim::BuildingKind::Extractor
                    && game.state.extractor_income(building.id)
                        == Some(oxide_sim::ExtractorIncome::Supported)
            })
            .expect("supported own Extractor")
            .id;
        let remote = game
            .state
            .buildings()
            .iter()
            .find(|building| {
                building.player == game.presentation.human
                    && building.kind == oxide_sim::BuildingKind::Extractor
                    && game.state.extractor_income(building.id)
                        == Some(oxide_sim::ExtractorIncome::Remote)
            })
            .expect("remote own Extractor")
            .id;
        let foreign = game
            .state
            .buildings()
            .iter()
            .find(|building| {
                building.player != game.presentation.human
                    && building.kind == oxide_sim::BuildingKind::Extractor
                    && game.state.extractor_income(building.id)
                        == Some(oxide_sim::ExtractorIncome::Supported)
            })
            .expect("supported foreign Extractor")
            .id;

        game.presentation.selection.buildings = vec![supported];
        let links = selected_economy_support_links(&game.view());
        assert!(!links.is_empty());
        assert!(links.iter().all(|link| link.extractor == supported));

        let foundry = links[0].foundry;
        game.presentation.selection.buildings = vec![foundry];
        let from_foundry = selected_economy_support_links(&game.view());
        assert!(from_foundry.contains(&EconomySupportLink {
            extractor: supported,
            foundry,
        }));
        assert!(
            from_foundry.iter().all(|link| link.extractor != remote),
            "remote Extractors cannot gain a decorative connection"
        );

        game.presentation.selection.buildings = vec![remote];
        assert!(selected_economy_support_links(&game.view()).is_empty());

        game.presentation.selection.buildings = vec![foreign];
        assert!(
            selected_economy_support_links(&game.view()).is_empty(),
            "foreign support state must not be disclosed by a link"
        );
    }

    #[test]
    fn production_progress_is_private_except_in_an_omniscient_view() {
        let mut game = Game::with_viewport(oxide_sim::Scenario::skirmish(), vec2(1280.0, 800.0))
            .expect("embedded skirmish builds");
        let own = game
            .state
            .buildings()
            .iter()
            .find(|building| building.player == game.presentation.human)
            .expect("the human has a Foundry");
        let hostile = game
            .state
            .buildings()
            .iter()
            .find(|building| building.player != game.presentation.human)
            .expect("the opponent has a Foundry");
        assert!(production_progress_visible(&game.view(), own));
        assert!(!production_progress_visible(&game.view(), hostile));

        game.presentation.spectate = true;
        assert!(production_progress_visible(&game.view(), hostile));
    }

    #[test]
    fn repair_bay_uses_the_exact_footprint_offset_aura() {
        let anchor = vec2(10.0, 20.0);
        let mut ranges = Vec::new();
        visit_building_ranges(anchor, oxide_sim::BuildingKind::RepairBay, 0, |range| {
            ranges.push(range);
        });

        let [range] = ranges.as_slice() else {
            panic!("Repair Bay should expose exactly one range: {ranges:?}");
        };
        assert_eq!(range.kind, BuildingRangeKind::Repair);
        let radius = oxide_sim::stats::REPAIR_BAY_RADIUS.to_num::<f32>();
        let size = oxide_sim::BuildingKind::RepairBay.base_stats().size;
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
    fn foundries_and_extractors_use_the_exact_square_support_footprint() {
        let anchor = vec2(10.0, 20.0);
        for kind in [
            oxide_sim::BuildingKind::Foundry,
            oxide_sim::BuildingKind::Extractor,
        ] {
            let mut ranges = Vec::new();
            visit_building_ranges(anchor, kind, 0, |range| ranges.push(range));
            let support = ranges
                .iter()
                .find(|range| range.kind == BuildingRangeKind::EconomySupport)
                .expect("economic endpoints expose their support footprint");
            let radius = footprint_distance_aura(oxide_sim::stats::EXTRACTOR_SUPPORT_RADIUS);
            let size = kind.base_stats().size;
            let footprint_max = anchor + vec2(size.0 as f32, size.1 as f32);
            assert_eq!(
                support.shape,
                BuildingRangeShape::FootprintSquare {
                    min: anchor,
                    max: footprint_max,
                    radius,
                }
            );
            assert_eq!(
                square_footprint_path(anchor, footprint_max, radius),
                [
                    anchor - vec2(radius, radius),
                    vec2(footprint_max.x + radius, anchor.y - radius),
                    footprint_max + vec2(radius, radius),
                    vec2(anchor.x - radius, footprint_max.y + radius),
                    anchor - vec2(radius, radius),
                ],
                "Chebyshev support has square corners"
            );
        }
    }

    #[test]
    fn weapon_ranges_remain_centered_circles() {
        let anchor = vec2(10.0, 20.0);
        let kind = oxide_sim::BuildingKind::Turret;
        let mut ranges = Vec::new();
        visit_building_ranges(anchor, kind, 0, |range| ranges.push(range));

        let weapon = ranges
            .iter()
            .find(|range| range.kind == BuildingRangeKind::Weapon)
            .expect("a Turret exposes its weapon range");
        let size = kind.base_stats().size;
        assert_eq!(
            weapon.shape,
            BuildingRangeShape::Circle {
                center: anchor + vec2(size.0 as f32, size.1 as f32) * 0.5,
                radius: kind.base_stats().weapons[0].range.to_num::<f32>(),
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
        visit_building_ranges(anchor, kind, 0, |range| ranges.push(range));

        let dead_zone = ranges
            .iter()
            .find(|range| range.kind == BuildingRangeKind::DeadZone)
            .expect("a Bastion exposes its close-pressure counter");
        let size = kind.base_stats().size;
        assert_eq!(
            dead_zone.shape,
            BuildingRangeShape::Circle {
                center: anchor + vec2(size.0 as f32, size.1 as f32) * 0.5,
                radius: kind.base_stats().weapons[0].minimum_range.to_num::<f32>(),
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
    fn curved_ranges_have_distinct_textures_and_all_ranges_have_distinct_icons() {
        let kinds = [
            BuildingRangeKind::Weapon,
            BuildingRangeKind::DeadZone,
            BuildingRangeKind::Vision,
            BuildingRangeKind::Radar,
            BuildingRangeKind::Repair,
            BuildingRangeKind::EconomySupport,
        ];
        let strokes: Vec<_> = kinds
            .iter()
            .copied()
            .filter(|kind| *kind != BuildingRangeKind::EconomySupport)
            .map(range_stroke)
            .collect();
        for (index, stroke) in strokes.iter().enumerate() {
            assert!(
                strokes[..index].iter().all(|other| other != stroke),
                "{stroke:?} was reused for two range meanings"
            );
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
    fn anti_air_buildings_use_the_weapon_domain_at_every_tier_and_in_placement() {
        let kind = oxide_sim::BuildingKind::FlakTurret;
        for (tier, stats) in kind.tiers().iter().enumerate() {
            let mut ranges = Vec::new();
            visit_building_ranges(vec2(10.0, 10.0), kind, tier as u8, |range| {
                ranges.push(range)
            });
            let weapon = ranges
                .iter()
                .find(|range| range.kind == BuildingRangeKind::AirWeapon)
                .expect("Flak Turret and Burst Flak both expose anti-air reach");
            assert_eq!(
                range_icon(weapon.kind),
                crate::panel::CapabilityIcon::AirWeapon
            );
            assert_ne!(
                range_color(weapon.kind),
                range_color(BuildingRangeKind::Weapon)
            );
            assert_eq!(range_stroke(weapon.kind), RangeStroke::Solid);
            assert!(
                matches!(weapon.shape, BuildingRangeShape::Circle { radius, .. } if radius == stats.weapons[0].range.to_num::<f32>())
            );
        }
        let game = economy_support_game();
        let mut input = InputState::new();
        input.placing = Some(kind);
        let mut indicators = Vec::new();
        visit_active_ranges(&game.view(), &input, |indicator| indicators.push(indicator));
        assert!(
            indicators
                .iter()
                .any(|indicator| indicator.icon == crate::panel::CapabilityIcon::AirWeapon)
        );
        assert!(
            !indicators
                .iter()
                .any(|indicator| indicator.icon == crate::panel::CapabilityIcon::Weapon)
        );
    }

    #[test]
    fn flakhound_and_sentinel_use_consistent_colors_and_marks_for_each_target_domain() {
        let scenario: oxide_sim::Scenario = serde_json::from_value(serde_json::json!({
            "name": "Weapon indicator fixture", "seed": 1,
            "map": ["....................", "....................", "..1.................",
                "....................", "....................", "....................",
                "....................", "....................", "....................",
                "....................", "....................", "...................."],
            "players": [{"name": "You", "faction": "ferrous", "scrap": 0, "bot": false}],
            "units": [{"player": 0, "kind": "flakhound", "x": 5, "y": 7},
                {"player": 0, "kind": "sentinel", "x": 9, "y": 7}]
        }))
        .unwrap();
        let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
        for kind in [
            oxide_sim::UnitKind::Flakhound,
            oxide_sim::UnitKind::Sentinel,
        ] {
            let id = game
                .state
                .units()
                .iter()
                .find(|unit| unit.kind == kind)
                .unwrap()
                .id;
            game.presentation.selection.units = vec![id];
            let mut indicators = Vec::new();
            visit_active_ranges(&game.view(), &InputState::new(), |indicator| {
                indicators.push(indicator)
            });
            let weapons: Vec<_> = indicators
                .iter()
                .filter(|indicator| {
                    matches!(
                        indicator.range.kind,
                        BuildingRangeKind::Weapon | BuildingRangeKind::AirWeapon
                    )
                })
                .collect();
            assert!(
                weapons
                    .iter()
                    .all(|indicator| indicator.color == range_color(indicator.range.kind))
            );
            assert!(
                weapons
                    .iter()
                    .any(|indicator| indicator.icon == crate::panel::CapabilityIcon::AirWeapon)
            );
            if kind == oxide_sim::UnitKind::Sentinel {
                assert!(
                    weapons
                        .iter()
                        .any(|indicator| indicator.icon == crate::panel::CapabilityIcon::Weapon)
                );
                assert_eq!(weapons.len(), 2);
                assert_ne!(weapons[0].color, weapons[1].color);
            } else {
                assert_eq!(weapons.len(), 1);
            }
        }
    }

    #[test]
    fn dots_keep_exact_spacing_across_path_vertices_and_ui_scales() {
        for scale in [0.75, 1.0, 2.0] {
            for vertices in [vec![0.0, 30.0], vec![0.0, 1.0, 7.0, 7.0, 19.0, 30.0]] {
                let path: Vec<_> = vertices.into_iter().map(|x| vec2(x * scale, 0.0)).collect();
                let mut spans: Vec<(f32, f32)> = Vec::new();
                visit_stroke_segments(&path, RangeStroke::Dotted, scale, |a, b| {
                    if let Some(last) = spans.last_mut()
                        && (last.1 - a.x).abs() < 0.0001
                    {
                        last.1 = b.x;
                    } else {
                        spans.push((a.x, b.x));
                    }
                });
                let expected = [(0.0, 2.5), (9.0, 11.5), (18.0, 20.5), (27.0, 29.5)];
                assert_eq!(spans.len(), expected.len());
                for ((a, b), (start, end)) in spans.into_iter().zip(expected) {
                    assert!((a - start * scale).abs() < 0.0001);
                    assert!((b - end * scale).abs() < 0.0001);
                }
            }
        }
    }

    #[test]
    fn range_fades_preserve_the_exact_outer_edge_and_stay_inside_small_ranges() {
        for radius in [2.0, 32.0, 640.0] {
            let shape = BuildingRangeShape::Circle {
                center: vec2(10.0, 20.0),
                radius,
            };
            let mesh = range_fade_mesh(shape, 10.0, WHITE);
            let count = mesh.vertices.len() / 3;
            assert_eq!(count, range_path(shape, 0.0).len());
            for (index, vertex) in mesh.vertices.iter().enumerate() {
                let distance = (vertex.position.truncate() - vec2(10.0, 20.0)).length();
                assert!(distance <= radius + 0.001);
                assert!(distance >= radius * 0.5 - 0.001);
                if index < count {
                    assert!((distance - radius).abs() < 0.001);
                }
                if index >= count * 2 {
                    assert_eq!(vertex.color[3], 0);
                }
            }
            assert!(
                mesh.indices
                    .iter()
                    .all(|index| (*index as usize) < mesh.vertices.len())
            );
        }
    }

    #[test]
    fn connection_stops_outside_both_footprints() {
        let a = (vec2(0.0, 0.0), vec2(20.0, 20.0));
        let b = (vec2(40.0, 0.0), vec2(60.0, 20.0));
        assert_eq!(
            footprint_link(a, b, 3.0),
            Some([vec2(23.0, 10.0), vec2(37.0, 10.0)])
        );
        let diagonal = (vec2(40.0, 40.0), vec2(60.0, 60.0));
        let [start, end] = footprint_link(a, diagonal, 3.0).unwrap();
        assert!((start - vec2(23.0, 23.0)).length() < 0.0001);
        assert!((end - vec2(37.0, 37.0)).length() < 0.0001);
        assert_eq!(footprint_link(a, a, 3.0), None);
        assert_eq!(
            footprint_link(a, (vec2(20.0, 0.0), vec2(40.0, 20.0)), 3.0),
            None
        );
    }

    #[test]
    fn range_clipping_filters_distant_buildings_and_reuses_interval_storage() {
        let path = circle_path(Vec2::ZERO, 160.0);
        let mut buildings: Vec<_> = (0..1000)
            .map(|i| {
                let min = vec2(2000.0 + i as f32 * 40.0, 2000.0);
                (min, min + Vec2::splat(32.0))
            })
            .collect();
        for i in 0..40 {
            let angle = std::f32::consts::TAU * i as f32 / 40.0;
            let center = vec2(angle.cos(), angle.sin()) * 160.0;
            buildings.push((center - Vec2::splat(10.0), center + Vec2::splat(10.0)));
        }
        let mut clipper = RangeClipper::new(&path, &buildings);
        assert_eq!(clipper.occluders.len(), 40);
        let storage = clipper.intervals.as_ptr();
        let capacity = clipper.intervals.capacity();
        let mut clipped = 0;
        for pair in path.windows(2) {
            clipper.visit_visible(pair[0], pair[1], |_, _| {});
            clipped += usize::from(!clipper.intervals.is_empty());
            assert_eq!(clipper.intervals.as_ptr(), storage);
            assert_eq!(clipper.intervals.capacity(), capacity);
        }
        assert_eq!(clipped, 240);
    }

    #[test]
    fn filtered_range_clipping_preserves_visible_spans_and_pattern_phase() {
        let buildings = [
            (vec2(-30.0, -10.0), vec2(0.0, 25.0)),
            (vec2(-20.0, -5.0), vec2(10.0, 35.0)),
            (vec2(-20.0, -5.0), vec2(10.0, 35.0)),
            (vec2(25.0, -40.0), vec2(40.0, 30.0)),
            (vec2(1000.0, 1000.0), vec2(1032.0, 1032.0)),
        ];
        for path in [
            circle_path(Vec2::ZERO, 30.0),
            rounded_footprint_path(Vec2::ZERO, vec2(20.0, 30.0), 15.0),
            square_footprint_path(Vec2::ZERO, vec2(20.0, 30.0), 15.0).to_vec(),
            vec![vec2(-50.0, 0.0), vec2(50.0, 0.0), vec2(-50.0, 0.0)],
        ] {
            for stroke in [
                RangeStroke::Solid,
                RangeStroke::ShortDash,
                RangeStroke::LongDash,
                RangeStroke::Dotted,
                RangeStroke::DashDot,
            ] {
                let mut clipper = RangeClipper::new(&path, &buildings);
                visit_stroke_segments(&path, stroke, 1.0, |a, b| {
                    let mut intervals: Vec<_> = buildings
                        .iter()
                        .filter_map(|&(min, max)| line_footprint_interval(a, b, min, max))
                        .collect();
                    intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
                    let mut expected = Vec::new();
                    let mut cursor: f32 = 0.0;
                    for (start, end) in intervals.into_iter().chain(std::iter::once((1.0, 1.0))) {
                        if cursor < start {
                            expected.push((a.lerp(b, cursor), a.lerp(b, start)));
                        }
                        cursor = cursor.max(end);
                    }
                    let mut actual = Vec::new();
                    clipper.visit_visible(a, b, |a, b| actual.push((a, b)));
                    assert_eq!(actual, expected);
                });
            }
        }
    }

    #[test]
    fn range_occluders_exclude_offscreen_and_unseen_buildings() {
        let mut game =
            Game::with_viewport(oxide_sim::Scenario::skirmish(), vec2(64.0, 64.0)).unwrap();
        let hostile = game
            .state
            .buildings()
            .iter()
            .find(|building| building.player != game.presentation.human)
            .unwrap();
        assert!(!hostile.tiles().any(|tile| game.my_vision().visible(tile)));
        let (width, height) = hostile.stats().size;
        game.presentation.camera.center = vec2(
            hostile.anchor.x as f32 + width as f32 * 0.5,
            hostile.anchor.y as f32 + height as f32 * 0.5,
        );
        assert!(range_occluders(&game.view()).is_empty());
        game.presentation.spectate = true;
        assert!(!range_occluders(&game.view()).is_empty());
        game.presentation.camera.center = vec2(-1000.0, -1000.0);
        assert!(range_occluders(&game.view()).is_empty());
    }

    #[test]
    fn support_line_occlusion_handles_crossings_misses_and_reversed_edges() {
        let min = vec2(4.0, 4.0);
        let max = vec2(6.0, 6.0);
        assert_eq!(
            line_footprint_interval(vec2(0.0, 5.0), vec2(10.0, 5.0), min, max),
            Some((0.4, 0.6))
        );
        assert_eq!(
            line_footprint_interval(vec2(10.0, 5.0), vec2(0.0, 5.0), min, max),
            Some((0.4, 0.6))
        );
        assert_eq!(
            line_footprint_interval(Vec2::ZERO, vec2(10.0, 0.0), min, max),
            None
        );
        assert_eq!(
            line_footprint_interval(Vec2::ZERO, vec2(3.0, 3.0), min, max),
            None
        );
        assert_eq!(
            line_footprint_interval(vec2(5.0, 5.0), vec2(5.0, 10.0), min, max),
            Some((0.0, 0.2))
        );
    }

    #[test]
    fn moth_payloads_begin_in_six_rack_positions_and_keep_their_impact_points() {
        let launch = vec2(5.0, 7.0);
        let heading = vec2(1.0, 0.0);
        let mut starts = Vec::new();
        for slot in 0..6 {
            let impact = vec2(7.0 + slot as f32 * 0.8, 7.1);
            let release = crate::game::LaunchPose {
                heading,
                kind: oxide_sim::UnitKind::Moth,
                slot,
            };
            let (start, direction) = moth_bomb_pose(launch, impact, release, 0.0, 10.0);
            assert!(direction.dot(heading) > 0.99);
            assert!(!starts.contains(&start));
            starts.push(start);
            for progress in [0.0, 0.01, 0.2, 0.5, 0.99, 1.0] {
                let (position, direction) = moth_bomb_pose(launch, impact, release, progress, 10.0);
                assert!(position.is_finite() && direction.is_finite());
            }
            assert!(
                moth_bomb_pose(launch, impact, release, 1.0, 10.0)
                    .0
                    .distance(impact)
                    < 1e-5
            );
        }
    }

    #[test]
    fn condor_payload_clears_the_nose_and_arrives_without_a_loft() {
        let launch = vec2(5.0, 7.0);
        let heading = vec2(1.0, 0.0);
        for impact in [launch, launch + vec2(0.1, 0.0), launch + vec2(3.0, 0.8)] {
            let (start, tangent) = condor_bomb_pose(launch, impact, heading, 0.0);
            assert!(start.is_finite() && tangent.is_finite());
            if impact != launch {
                assert!(start.x > launch.x);
                assert_eq!(start.y, launch.y);
                assert!(tangent.dot(heading) > 0.99);
            }
            let mut previous = start;
            for step in 1..=100 {
                let (position, direction) =
                    condor_bomb_pose(launch, impact, heading, step as f32 / 100.0);
                assert!(position.is_finite() && direction.is_finite());
                assert!(position.x >= previous.x);
                assert!(position.y >= launch.y && position.y <= impact.y);
                previous = position;
            }
            assert!(previous.distance(impact) < 1e-5);
        }
    }

    #[test]
    fn artillery_shells_begin_at_the_barrel_and_use_a_low_arc() {
        let launch = vec2(5.0, 7.0);
        let impact = vec2(15.0, 7.0);
        let shooter = oxide_sim::Target::Unit(oxide_sim::UnitId(4));
        assert_eq!(
            shell_visual_origin(launch, impact, shooter, oxide_sim::ProjectileKind::Bomb),
            launch
        );
        assert_eq!(
            shell_visual_origin(launch, impact, shooter, oxide_sim::ProjectileKind::Missile),
            launch + vec2(0.53, 0.0)
        );
        let from = shell_visual_origin(
            launch,
            impact,
            oxide_sim::Target::Building(oxide_sim::BuildingId(4)),
            oxide_sim::ProjectileKind::Shell,
        );
        assert!((from.x - 5.98).abs() < 1.0e-4);
        assert_eq!(from.y, launch.y);
        let bombard_from = shell_visual_origin(
            launch,
            impact,
            oxide_sim::Target::Unit(oxide_sim::UnitId(4)),
            oxide_sim::ProjectileKind::Shell,
        );
        assert!((bombard_from.x - 5.254_297).abs() < 1.0e-4);
        assert_eq!(bombard_from.y, launch.y);
        assert_eq!(
            shell_visual_origin(
                launch,
                launch,
                oxide_sim::Target::Unit(oxide_sim::UnitId(4)),
                oxide_sim::ProjectileKind::Shell,
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
        assert!((tail - 0.486).abs() < 1.0e-4);
        assert!((0.5 - tail) * 10.0 <= 0.140_001);
    }

    #[test]
    fn bombard_payload_keeps_a_straight_constant_speed_course() {
        let launch = vec2(5.0, 7.0);
        for angle in [0.0_f32, 0.7, 1.57, 2.9, 4.71] {
            let heading = vec2(angle.cos(), angle.sin());
            let impact = launch + vec2((angle + 0.04).cos(), (angle + 0.04).sin()) * 10.0;
            let start = bombard_shell_position(launch, impact, heading, 0.0);
            let next = bombard_shell_position(launch, impact, heading, 0.001);
            assert!((start - launch).normalize().dot(heading) > 0.999);
            assert!((next - start).normalize().dot(heading) > 0.999);
            assert!(bombard_shell_position(launch, impact, heading, 1.0).distance(impact) < 1e-5);
            for step in 0..=100 {
                let t = step as f32 / 100.0;
                let position = bombard_shell_position(launch, impact, heading, t);
                assert!(position.is_finite());
                assert!(position.distance(start.lerp(impact, t)) < 1e-5);
            }
        }
    }

    #[test]
    fn flakhound_reports_four_barrels_as_two_offset_pairs() {
        let delay = crate::game::FlakYokeDelay::OneTick;
        let first = flak_barrel_rounds(0.0, delay, 2);
        assert_eq!(first.iter().flatten().count(), 2);
        assert!(first[..2].iter().all(Option::is_some));
        let both = flak_barrel_rounds(delay.seconds(), delay, 2);
        let rounds: Vec<_> = both.into_iter().flatten().collect();
        assert_eq!(rounds.len(), 4);
        assert!(rounds.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert_eq!(rounds[0].1, rounds[1].1);
        assert_eq!(rounds[2].1, rounds[3].1);
        assert!(rounds[0].1 > rounds[2].1);
        assert_eq!(
            flak_barrel_rounds(0.0, delay, 1).iter().flatten().count(),
            1
        );
    }

    #[test]
    fn flak_turret_rounds_match_both_barrel_banks_and_upgrade() {
        let delay = crate::game::FlakYokeDelay::OneAndHalfTicks;
        for count in [2, 3] {
            let first = flak_barrel_rounds(0.0, delay, count);
            assert_eq!(first.iter().flatten().count(), usize::from(count));
            assert!(first.iter().flatten().all(|round| round.0 < 0.0));
            let both: Vec<_> = flak_barrel_rounds(delay.seconds(), delay, count)
                .into_iter()
                .flatten()
                .collect();
            assert_eq!(both.len(), usize::from(count) * 2);
            assert!(both.windows(2).all(|pair| pair[0].0 < pair[1].0));
            for index in 0..usize::from(count) {
                assert_eq!(both[index].0, -both[both.len() - 1 - index].0);
                assert!(both[index].1 > both[index + usize::from(count)].1);
            }
            assert!(
                flak_barrel_rounds(delay.seconds() + FLAK_ROUND_TRAVEL + 0.01, delay, count)
                    .iter()
                    .all(Option::is_none)
            );
        }
    }

    #[test]
    fn missiles_eject_then_accelerate_without_changing_arrival() {
        for distance in [0.0, 0.1, 1.0, 6.0, 20.0] {
            let total = (distance / 0.30_f32).ceil().max(1.0);
            assert_eq!(missile_travel_progress(0.0, total, distance), 0.0);
            assert!((missile_travel_progress(1.0, total, distance) - 1.0).abs() < 1.0e-5);
            let mut previous = 0.0;
            for step in 0..=200 {
                let progress = missile_travel_progress(step as f32 / 200.0, total, distance);
                assert!(progress.is_finite() && (0.0..=1.0).contains(&progress));
                assert!(progress >= previous);
                previous = progress;
            }
        }
        let total = 40.0;
        let at = |tick| missile_travel_progress(tick / total, total, 12.0) * 12.0;
        assert!((at(3.0) - 0.45).abs() < 1.0e-5);
        assert_eq!(missile_motor_strength(3.0 / total, total), 0.0);
        assert_eq!(missile_motor_strength(5.0 / total, total), 1.0);
        assert!(at(5.0) - at(4.0) > at(2.0) - at(1.0));
        assert!(at(7.0) - at(6.0) > at(5.0) - at(4.0));
        assert!((at(3.0001) - at(3.0)).abs() < 0.001);
        assert!((at(5.0001) - at(5.0)).abs() < 0.001);
    }

    #[test]
    fn separate_flak_yokes_report_at_their_authored_delays() {
        use crate::game::FlakYokeDelay;

        let unit_delay = FlakYokeDelay::OneTick;
        assert_eq!(flak_round_progress(0.0, unit_delay), [Some(0.0), None]);

        let both = flak_round_progress(unit_delay.seconds(), unit_delay);
        assert!(both[0].is_some());
        assert_eq!(both[1], Some(0.0));

        let after_first = flak_round_progress(FLAK_ROUND_TRAVEL + 1.0e-4, unit_delay);
        assert!(after_first[0].is_none());
        assert!(after_first[1].is_some());

        let turret_delay = FlakYokeDelay::OneAndHalfTicks.seconds();
        assert_eq!(turret_delay, 1.5 * crate::game::TICK_DT);
        assert_eq!(
            flak_round_progress(0.0, FlakYokeDelay::None),
            [Some(0.0), Some(0.0)]
        );
    }

    #[test]
    fn forge_spot_reaches_its_target_before_the_effect_expires() {
        assert_eq!(forge_spot_phases(0.0), (0.0, 0.0));
        assert_eq!(forge_spot_phases(FORGE_SPOT_TRAVEL_FRACTION), (1.0, 0.0));
        let (travel, impact) = forge_spot_phases(0.99);
        assert_eq!(travel, 1.0);
        assert!(impact > 0.9);
    }

    #[test]
    fn kinetic_reports_reach_impact_before_fading_and_respect_fog() {
        use crate::game::ShotStyle;
        for heavy in [false, true] {
            let style = ShotStyle::Kinetic { heavy };
            assert_eq!(shot_impact_progress(style, 0.0), 0.0);
            assert_eq!(shot_impact_progress(style, style.life() * 0.25), 0.0);
            assert!(shot_impact_progress(style, style.life() * 0.9) > 0.5);
            assert_eq!(shot_impact_progress(style, style.life()), 1.0);
            assert_eq!(
                shot_visibility(style, false, true),
                ShotVisibility::ImpactOnly
            );
            assert_eq!(shot_visibility(style, true, false), ShotVisibility::Hidden);
        }
    }

    #[test]
    fn shot_visibility_never_points_into_fog() {
        use crate::game::ShotStyle;

        assert_eq!(
            shot_visibility(ShotStyle::Rail, true, true),
            ShotVisibility::Full
        );
        assert_eq!(
            shot_visibility(ShotStyle::ForgeSpot, false, true),
            ShotVisibility::ImpactOnly
        );
        assert_eq!(
            shot_visibility(ShotStyle::Contact, true, true),
            ShotVisibility::ImpactOnly
        );
        assert_eq!(
            shot_visibility(ShotStyle::ForgeSpot, true, false),
            ShotVisibility::Hidden
        );
    }

    #[test]
    fn sapper_visibility_separates_the_source_body_from_the_impact_bloom() {
        assert_eq!(
            sapper_effect_visibility(false, false, true),
            SapperEffectVisibility {
                body: false,
                bloom: true,
            }
        );
        assert_eq!(
            sapper_effect_visibility(false, true, false),
            SapperEffectVisibility {
                body: true,
                bloom: false,
            }
        );
        assert_eq!(
            sapper_effect_visibility(false, false, false),
            SapperEffectVisibility {
                body: false,
                bloom: false,
            }
        );
        assert_eq!(
            sapper_effect_visibility(true, false, false),
            SapperEffectVisibility {
                body: true,
                bloom: true,
            }
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
