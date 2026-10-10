//! Everything that lives on the ground: buildings, units, effects,
//! pings, range rings, radar blips, rally lines, breadcrumbs, the
//! placement ghost, and the drag rectangle.

use super::*;
use crate::numeric;
use crate::numeric::Fit;
use crate::render::prim::{fill_circle, line_between, stroke_circle};

/// The armed building follows the cursor as a translucent footprint. The
/// tint and the command share the queue-aware placement verdict, so what
/// looks legal is legal: green founds immediately, amber founds on arrival
/// (part of the footprint is remembered ground, judged from memory rather
/// than live state so the tint cannot reveal hidden enemies), and red is
/// refused.
pub(crate) fn draw_placement_ghost(
    game: &crate::game::Scene<'_>,
    sprites: &Sprites,
    input: &InputState,
) {
    let Some((kind, anchor)) = crate::input::placement_preview_anchor(game, input) else {
        return;
    };
    let zoom = game.presentation.camera.zoom;
    let (w, h) = kind.size();
    let queue = input.placing_stroke.is_some() || input.queue_held();
    let ok = crate::input::placement_refusal(game, kind, anchor, queue).is_none();
    let screen = game
        .presentation
        .camera
        .to_screen(vec2(anchor.x as f32, anchor.y as f32));
    let dest = vec2(w as f32 * zoom, h as f32 * zoom);
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
            source: Some(sprites.construction(kind, 0, 0)),
            ..Default::default()
        },
    );
    if ok && input.queue_held() {
        let corner = screen + vec2(dest.x, 0.0);
        fill_circle(corner, 9.0, Color::new(0.08, 0.08, 0.1, 0.85));
        draw_queued_mark(corner, 5.0, BONE);
    }
}

/// The plus a queued order wears, on its ping and on the placement ghost:
/// it joins the program rather than replacing it.
fn draw_queued_mark(center: Vec2, arm: f32, color: Color) {
    line_between(center - vec2(arm, 0.0), center + vec2(arm, 0.0), 2.0, color);
    line_between(center - vec2(0.0, arm), center + vec2(0.0, arm), 2.0, color);
}

/// An inspected salvage tile wears a selected building's outline.
pub(crate) fn draw_selected_pile(game: &crate::game::Scene<'_>) {
    let Some(tile) = game.presentation.selection.pile else {
        return;
    };
    let zoom = game.presentation.camera.zoom;
    let screen = game
        .presentation
        .camera
        .to_screen(vec2(tile.x as f32, tile.y as f32));
    draw_rectangle_lines(
        screen.x - 2.0,
        screen.y - 2.0,
        zoom + 4.0,
        zoom + 4.0,
        3.0,
        BONE,
    );
}

/// Paid provisional scaffolds remain faint amber footprints until their
/// ground has been verified.
pub(crate) fn draw_pending_founds(game: &crate::game::Scene<'_>, sprites: &Sprites) {
    let zoom = game.presentation.camera.zoom;
    for site in game
        .state
        .buildings()
        .iter()
        .filter(|b| b.player == game.presentation.human && b.provisional())
    {
        let (w, h) = site.kind.size();
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
                source: Some(sprites.construction(site.kind, 0, 0)),
                ..Default::default()
            },
        );
    }
}

/// The screen-space waypoints one selected unit's program draws, as the
/// staged commands will leave it. Pure, so the fog rules are testable: a
/// unit outside the decorated selection or a foreign unit yields no
/// points, because an ally's or enemy's orders are intent the viewer may
/// not read. An own walk draws at the tile its player clicked, explored
/// or not, never at the slot or endpoint the simulation resolved around
/// it. Each verb draws in its own color.
pub(crate) fn breadcrumb_points(
    game: &crate::game::Scene<'_>,
    projection: &crate::game::projection::Projection,
    unit: &oxide_sim::Unit,
) -> Vec<(usize, Vec2, Color)> {
    use crate::game::world_vec;
    use crate::input::tile_center;
    if unit.player != game.presentation.human {
        return Vec::new();
    }
    let Some(program) = projection.program(unit.id) else {
        return Vec::new();
    };
    let verb_color = |order: &oxide_sim::Order| match order {
        oxide_sim::Order::Run { .. } => BONE_FAINT,
        oxide_sim::Order::Advance { .. } => Color::new(0.95, 0.76, 0.28, 0.62),
        // A chase (at a victim) and a fighting march (toward ground)
        // draw in different colors.
        oxide_sim::Order::Attack { .. } => Color::new(0.85, 0.32, 0.29, 0.55),
        oxide_sim::Order::Hunt { .. } => Color::new(0.88, 0.55, 0.26, 0.55),
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
    // Building targets draw at their footprint's center, which lies on a
    // tile seam when the footprint is even, so they are never tile-snapped.
    let goal_of = |order: &oxide_sim::Order| {
        let goal = match order {
            oxide_sim::Order::Run { goal }
            | oxide_sim::Order::Advance { goal }
            | oxide_sim::Order::Hunt { goal } => tile_center(goal.tile()),
            oxide_sim::Order::Harvest { node, .. } => tile_center(*node),
            oxide_sim::Order::ReturnCargo { foundry, .. } => {
                world_vec(game.state.building(*foundry)?.center())
            }
            oxide_sim::Order::Build { site } => {
                world_vec(projection.building(game.state, *site)?.center())
            }
            oxide_sim::Order::Found { kind, anchor } => {
                world_vec(oxide_sim::geometry::footprint_center(*anchor, kind.size()))
            }
            oxide_sim::Order::Repair { building } | oxide_sim::Order::Salvage { building } => {
                world_vec(game.state.building(*building)?.center())
            }
            // A repair patient is the viewer's own machine, so it is always seen.
            oxide_sim::Order::RepairUnit { unit } => tile_center(game.state.unit(*unit)?.tile()),
            oxide_sim::Order::Board { transport } => {
                tile_center(game.state.unit(*transport)?.tile())
            }
            oxide_sim::Order::Unload { at, .. } => tile_center(at.tile()),
            // A landing that took over a walk marks the walk's click.
            oxide_sim::Order::Land { goal, from } => tile_center(from.unwrap_or(*goal)),
            oxide_sim::Order::Attack { target, .. } => world_vec(
                game.state
                    .attack_view(game.presentation.human, *target)?
                    .position,
            ),
            oxide_sim::Order::Idle => return None,
        };
        Some((goal, verb_color(order)))
    };
    // Each point carries its program position (0 = the active order,
    // i = queue[i-1]), the order the dock lists chips in, so a leg whose
    // target the viewer can no longer place (a lost contact, a razed
    // building) leaves a numbering gap instead of renumbering the rest
    // out of agreement with the chips.
    let mut points: Vec<(usize, Vec2, Color)> = Vec::new();
    for (i, order) in program.orders.iter().enumerate() {
        if let Some((g, c)) = goal_of(order) {
            points.push((i, game.presentation.camera.to_screen(g), c));
        }
    }
    points
}

/// The selected units that draw decor, subject first. The dock, portrait,
/// and full-strength trail all describe the subject, so it must never be
/// the entry the cap drops: a selection arrives in id order, and older
/// units ahead of it could push it past `DECOR_CAP`.
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

/// Queued waypoints of the selection, drawn as a faint chain; a patrol
/// closes the loop. While a patrol is being armed, the collected route
/// draws in scrap-amber instead.
pub(crate) fn draw_breadcrumbs(game: &crate::game::Scene<'_>, input: &InputState) {
    let dot = |p: Vec2, color: Color| fill_circle(p, 3.0, color);
    if let Some(route) = &input.patrol_route {
        let mut prev: Option<Vec2> = None;
        for tile in route {
            let p = game
                .presentation
                .camera
                .to_screen(vec2(tile.x as f32 + 0.5, tile.y as f32 + 0.5));
            if let Some(a) = prev {
                line_between(a, p, 1.5, SCRAP_COLOR);
            }
            dot(p, SCRAP_COLOR);
            prev = Some(p);
        }
        return;
    }
    // The subject's trail draws full strength and numbered, matching the
    // dock; the rest of the selection's trails dim.
    let subject = crate::panel::subject_unit(game);
    let projection = game.projection();
    for id in decor_units(game) {
        let (Some(unit), Some(program)) = (game.state.unit(id), projection.program(id)) else {
            continue;
        };
        let points = breadcrumb_points(game, &projection, unit);
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
        let numbered = is_subject && program.orders.len() > 1;
        let mut prev = start;
        for (idx, p, color) in &points {
            let color = fade(*color);
            line_between(prev, *p, 1.0, color);
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
        if program.looping && points.len() > 1 {
            let (_, first, color) = points[0];
            let color = fade(color);
            line_between(prev, first, 1.0, color);
        }
    }
}

fn production_progress_visible(
    game: &crate::game::Scene<'_>,
    building: &oxide_sim::Building,
) -> bool {
    building.player == game.presentation.human || game.presentation.all_seeing()
}

fn building_body_sources(
    sprites: &Sprites,
    kind: oxide_sim::BuildingKind,
    tier: u8,
    body: super::motion::BuildingBodyFrame,
) -> (Rect, Rect) {
    use super::motion::BuildingBodyFrame;
    if !matches!(body, BuildingBodyFrame::Construction { .. })
        && kind == oxide_sim::BuildingKind::Array
        && let Some(rig) = sprites.array_rig()
    {
        return rig.layers(tier)[0];
    }
    match body {
        BuildingBodyFrame::Idle => (
            sprites.building_tiered(kind, tier),
            sprites.building_tiered_accent(kind, tier),
        ),
        BuildingBodyFrame::Work(work) => (
            sprites.building_working(kind, tier, work + 1),
            sprites.building_working_accent(kind, tier, work + 1),
        ),
        BuildingBodyFrame::Construction { stage, phase } => (
            sprites.construction(kind, stage, phase),
            sprites.construction_accent(kind, stage, phase),
        ),
        BuildingBodyFrame::Action(action) => (
            sprites.building_action(kind, action),
            sprites.building_action_accent(kind, action),
        ),
    }
}

pub(super) fn strike_contact(
    game: &crate::game::Scene<'_>,
    sprites: &Sprites,
    surface: Option<crate::game::HitSurface>,
    from: Vec2,
    to: Vec2,
    style: crate::game::ShotStyle,
) -> Option<Vec2> {
    use crate::game::{HitSurface, ShotStyle};
    match surface? {
        HitSurface::Building(hit) => {
            let aim = if style == ShotStyle::Contact {
                to + (to - from).normalize_or_zero() * 0.15
            } else {
                to
            };
            building_contact(game, sprites, hit, from, aim)
        }
        HitSurface::Unit(hit) => {
            if let Some(unit) = game.state.unit(hit.id)
                && crate::strategic_markers::visible(game, unit)
            {
                let pose =
                    super::unit_body_pose(game, sprites, unit, game.presentation.tick_fraction());
                return posed_unit_contact(sprites, &pose, from, pose.center);
            }
            unit_contact(sprites, hit, from, hit.center)
        }
    }
}

pub(super) fn unit_contact(
    sprites: &Sprites,
    hit: crate::game::UnitHit,
    from: Vec2,
    aim: Vec2,
) -> Option<Vec2> {
    let rotate = |v: Vec2, angle: f32| {
        vec2(
            v.x * angle.cos() - v.y * angle.sin(),
            v.x * angle.sin() + v.y * angle.cos(),
        )
    };
    let body = hit.body;
    let center = hit.center;
    let source = super::unit_body_sources(sprites, body.kind, hit.frame).0;
    let size = super::unit_draw_scale(body.kind);
    sprites
        .sprite_contact(
            source,
            rotate(from - center, -body.rotation),
            rotate(aim - center, -body.rotation),
            Vec2::splat(-size * 0.5),
            Vec2::splat(size),
        )
        .map(|point| center + rotate(point, body.rotation))
}

fn posed_unit_contact(
    sprites: &Sprites,
    pose: &super::UnitBodyPose,
    from: Vec2,
    aim: Vec2,
) -> Option<Vec2> {
    let (source, center, size, rotation) =
        (pose.source, pose.center, pose.size, pose.body_rotation);
    let rotate = |v: Vec2, angle: f32| Vec2::from_angle(angle).rotate(v);
    sprites
        .sprite_contact(
            source,
            rotate(from - center, -rotation),
            rotate(aim - center, -rotation),
            Vec2::splat(-size * 0.5),
            Vec2::splat(size),
        )
        .map(|point| center + rotate(point, rotation))
}

fn payload_contact(
    game: &crate::game::Scene<'_>,
    sprites: &Sprites,
    index: usize,
) -> (Vec2, Option<crate::game::HitSurface>) {
    let shell = &game.state.shells()[index];
    let at = vec2(
        shell.impact.x.to_num::<f32>(),
        shell.impact.y.to_num::<f32>(),
    );
    let target = game
        .presentation
        .projectile_releases
        .flight(game.state.shells(), index)
        .and_then(|flight| flight.target);
    let surface =
        game.presentation
            .payload_surface(game.state, target, shell.player, at, shell.targets);
    let from = vec2(
        shell.launch.x.to_num::<f32>(),
        shell.launch.y.to_num::<f32>(),
    );
    let point = match surface {
        Some(crate::game::HitSurface::Unit(hit)) => {
            let unit = game
                .state
                .unit(hit.id)
                .filter(|unit| crate::strategic_markers::visible(game, unit));
            unit.and_then(|unit| {
                let pose =
                    super::unit_body_pose(game, sprites, unit, game.presentation.tick_fraction());
                posed_unit_contact(
                    sprites,
                    &pose,
                    if shell.kind == oxide_sim::ProjectileKind::Bomb {
                        at
                    } else {
                        from
                    },
                    at,
                )
                .filter(|point| on_payload_course(from, at, *point))
            })
        }
        _ => strike_contact(
            game,
            sprites,
            surface,
            if shell.kind == oxide_sim::ProjectileKind::Bomb {
                at
            } else {
                from
            },
            at,
            crate::game::ShotStyle::Rail,
        ),
    }
    .filter(|point| {
        game.presentation.all_seeing() || game.my_vision().visible(numeric::tile_at(*point))
    });
    (point.unwrap_or(at), surface)
}

pub(super) fn impact_contact(
    game: &crate::game::Scene<'_>,
    sprites: &Sprites,
    surface: Option<crate::game::HitSurface>,
    from: Vec2,
    at: Vec2,
    payload: oxide_sim::ProjectileKind,
) -> Vec2 {
    let ray_from = if payload == oxide_sim::ProjectileKind::Bomb {
        at
    } else {
        from
    };
    match surface {
        Some(crate::game::HitSurface::Unit(hit)) => unit_contact(sprites, hit, ray_from, at)
            .filter(|point| on_payload_course(from, at, *point)),
        _ => strike_contact(
            game,
            sprites,
            surface,
            ray_from,
            at,
            crate::game::ShotStyle::Rail,
        ),
    }
    .filter(|point| {
        game.presentation.all_seeing() || game.my_vision().visible(numeric::tile_at(*point))
    })
    .unwrap_or(at)
}

fn on_payload_course(from: Vec2, to: Vec2, contact: Vec2) -> bool {
    let ray = to - from;
    let t = (contact - from).dot(ray) / ray.length_squared().max(f32::EPSILON);
    (0.0..=1.0).contains(&t) && contact.distance(from + ray * t) < 0.01
}

fn payload_flight_ticks(game: &crate::game::Scene<'_>, index: usize) -> f32 {
    let shell = &game.state.shells()[index];
    game.presentation
        .projectile_releases
        .flight(game.state.shells(), index)
        .map_or_else(
            || shell.arrival.saturating_sub(shell.launched_at).max(1) as f32,
            |flight| flight.ticks as f32,
        )
}

pub(super) fn building_contact(
    game: &crate::game::Scene<'_>,
    sprites: &Sprites,
    hit: crate::game::BuildingHit,
    from: Vec2,
    aim: Vec2,
) -> Option<Vec2> {
    let hit = game
        .state
        .building(hit.id)
        .filter(|building| {
            game.presentation.all_seeing()
                || building.player == game.presentation.human
                || building.tiles().any(|tile| game.my_vision().visible(tile))
                    && game
                        .state
                        .building_apparent(game.presentation.human, building)
        })
        .map_or(hit, |building| {
            crate::game::BuildingHit::capture(game.state, building)
        });
    let animation = game.presentation.animations.building_state(
        hit.facts,
        crate::presentation_animation::AnimationClock::from_state(
            game.state,
            game.presentation.tick_fraction(),
        ),
        crate::presentation_animation::AnimationOptions {
            reduced_motion: reduced_motion(),
        },
    );
    let frame = super::motion::building_frame(hit.kind, animation);
    let (source, _) = building_body_sources(sprites, hit.kind, hit.tier, frame.body);
    let (width, height) = hit.kind.size();
    let size = vec2(width as f32, height as f32);
    let aim = if (aim - from).length_squared() < f32::EPSILON {
        hit.anchor + size * 0.5
    } else {
        aim
    };
    sprites.sprite_contact(source, from, aim, hit.anchor, size)
}

fn draw_defense_mount(
    game: &crate::game::Scene<'_>,
    sprites: &Sprites,
    building: &oxide_sim::Building,
    action: Option<usize>,
) {
    let draw = |x, y, tint, params| {
        sprites.draw_building(x, y, tint, params, game.presentation.camera.zoom);
    };
    let screen = game
        .presentation
        .camera
        .to_screen(vec2(building.anchor.x as f32, building.anchor.y as f32));
    let (width, height) = building.kind.size();
    let dest = vec2(
        width as f32 * game.presentation.camera.zoom,
        height as f32 * game.presentation.camera.zoom,
    );
    let source = match action {
        Some(frame) => sprites.defense_mount_action(building.kind, building.tier, frame),
        None => sprites.defense_mount(building.kind, building.tier),
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

#[expect(
    clippy::too_many_lines,
    reason = "one draw pass over every building and its dressing"
)]
pub(crate) fn draw_buildings(game: &crate::game::Scene<'_>, sprites: &Sprites) {
    const BUILDING_CULL_MARGIN: f32 = 4.5;
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
            let (w, h) = ghost.kind.size();
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
            // Unrefreshed memories fade with age; unstamped memories
            // (from loaded saves) start aging now.
            let age = {
                let mut seen = game.presentation.last_seen.borrow_mut();
                let stamp = *seen
                    .entry(key)
                    .or_insert_with(|| game.presentation.fx_time());
                game.presentation.fx_time() - stamp
            };
            let fade = 1.0 - super::staleness_fade(age);
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
                sprites.building(ghost.kind)
            } else {
                sprites.construction(ghost.kind, 0, 0)
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
                && let Some(mount) = sprites.defense_mount(ghost.kind, 0)
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
    // plus bars and site dressing.
    let (view_lo, view_hi) = game.presentation.camera.world_rect();
    for building in game.state.buildings().iter().filter(|b| !b.provisional()) {
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
        let screen = game.presentation.camera.to_screen(anchor);
        let (w, h) = building.kind.size();
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
        let array_layers = (building.built() && building.kind == oxide_sim::BuildingKind::Array)
            .then(|| sprites.array_rig())
            .flatten()
            .map(|rig| rig.layers(building.tier));
        let (source, accent_source) =
            building_body_sources(sprites, building.kind, building.tier, frame.body);
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
            let local_pivot = dest * vec2(0.5, 49.0 / 128.0);
            let pivot = screen + local_pivot;
            let layer_origin = pivot - local_pivot;
            let (source, accent) = layers[1];
            for (source, tint) in
                std::iter::once((source, WHITE)).chain(accent_tint.map(|tint| (accent, tint)))
            {
                draw(
                    layer_origin.x,
                    layer_origin.y,
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
        if building.built() && crate::look::defense(building.kind).is_some() {
            draw_defense_mount(game, sprites, building, frame.mount_action);
        }
        if !building.built() {
            // Construction progress in bone, distinct from training amber.
            let ticks = building.stats().construction.map_or(1, |c| c.build_ticks);
            let fraction = building.construction_progress().unwrap_or(0) as f32 / ticks as f32;
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
        // A site's partial hp is what the construction ramp grants, so the
        // progress bar shows it alone; the hp bar appears only when damage
        // has taken hp construction already gave. The check mirrors the
        // sim's integer ramp exactly (a float version flickers).
        let max_hp = building.stats().max_hp;
        let under_own_salvage = building.built() && salvaging.contains(&building.id);
        let wounded = if under_own_salvage {
            // The gold teardown bar below already shows the fraction.
            false
        } else if let Some(progress) = building.construction_progress() {
            let ticks = building.stats().construction.map_or(1, |c| c.build_ticks);
            let start = max_hp / 5;
            let expected = start + (max_hp - start) * progress.min(ticks) / ticks;
            building.hp < expected
        } else {
            building.hp < max_hp
        };
        if wounded {
            hp_bar(screen.x, screen.y - 8.0, dest.x, building.hp, max_hp);
        }
        // Production progress, drawn under the works.
        if production_progress_visible(game, building)
            && let Some(kind) = building.queue.front()
        {
            let fraction = building.training_progress() as f32 / kind.stats().train_ticks as f32;
            draw_rectangle(screen.x, screen.y + dest.y + 3.0, dest.x, 4.0, HP_BACK);
            draw_rectangle(
                screen.x,
                screen.y + dest.y + 3.0,
                dest.x * fraction,
                4.0,
                SCRAP_COLOR,
            );
        }
        // A teardown in progress draws remaining hp in scrap gold. Keyed on
        // an own crew's Order::Salvage, never on hp shape (shelling looks
        // identical), so enemy salvage shows nothing through the fog.
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
    // them. Each flyer casts an offset shadow so altitude reads even when
    // nothing overlaps.
    draw_unit_pass(game, sprites, alpha, oxide_sim::stats::Domain::Ground);
    draw_bomber_bombs(game, sprites);
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
            if !crate::look::fires(kind, oxide_sim::ProjectileKind::Bomb) {
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

fn draw_bomber_bombs(game: &crate::game::Scene<'_>, sprites: &Sprites) {
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
        let (impact, _) = payload_contact(game, sprites, index);
        let total = payload_flight_ticks(game, index);
        let t = (crate::audio_timeline::projectile_elapsed_ticks(now, shell.arrival, total)
            / total)
            .clamp(0.0, 1.0);
        let (position, direction) = condor_bomb_pose(launch, impact, release.heading, t);
        if !game.presentation.all_seeing()
            && game.state.hostile(game.presentation.human, shell.player)
            && !game.my_vision().visible(TilePos::new(
                numeric::to_i32(position.x.floor()),
                numeric::to_i32(position.y.floor()),
            ))
        {
            continue;
        }
        let flat = game.presentation.camera.to_screen(position);
        let lift = crate::look::unit(release.kind)
            .airframe
            .map_or(0.0, |airframe| airframe.lift);
        let center = flat - vec2(0.0, zoom * lift * (1.0 - t * t));
        let scale = zoom * (1.0 - 0.15 * t);
        let normal = vec2(-direction.y, direction.x);
        let nose = center + direction * scale * 0.14;
        let back = center - direction * scale * 0.14;
        draw_circle(
            flat.x + zoom * 0.10,
            flat.y + zoom * 0.14,
            scale * 0.065,
            Color::new(0.02, 0.02, 0.03, 0.35),
        );
        line_between(back, nose, scale * 0.15, Color::from_rgba(12, 13, 17, 255));
        line_between(
            back,
            nose,
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
        oxide_sim::Target::Building(_) => oxide_sim::BuildingKind::Bastion.size().0 as f32 * 0.49,
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
        // Bombard arcs visibly as indirect artillery, but its lift above
        // the flat path stays capped.
        oxide_sim::Target::Unit(_) => (screen_distance * 0.06).min(zoom * 0.60),
        // Bastion is a low-carriage siege gun; a high arc would detach its
        // compact shell from the barrel and impact.
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

fn direct_phases(style: crate::game::ShotStyle, age: f32) -> (f32, f32) {
    let travel_time = match style {
        crate::game::ShotStyle::ForgeSpot => 0.12,
        crate::game::ShotStyle::Mortar => 0.144,
        _ => return forge_spot_phases(age / style.life()),
    };
    (
        (age / travel_time).clamp(0., 1.),
        ((age - travel_time) / (style.life() - travel_time)).clamp(0., 1.),
    )
}

fn shot_impact_progress(style: crate::game::ShotStyle, age: f32) -> f32 {
    use crate::game::ShotStyle;
    match style {
        ShotStyle::Contact | ShotStyle::Rail => (age / style.life()).clamp(0.0, 1.0),
        ShotStyle::ForgeSpot | ShotStyle::Mortar | ShotStyle::Kinetic { .. } => {
            direct_phases(style, age).1
        }
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

#[expect(clippy::too_many_lines, reason = "draws every shell and effect kind")]
pub(crate) fn draw_fx(game: &crate::game::Scene<'_>, sprites: &Sprites) {
    let sees = |p: Vec2| game.my_vision().visible(numeric::tile_at(p));
    // Shells render from sim state, aged by sim ticks: pause holds them
    // mid-air, speed changes track, and a replay loaded mid-flight
    // restores them.
    let now = game.state.current_tick() as f32 + game.presentation.tick_fraction();
    for (index, shell) in game.state.shells().iter().enumerate() {
        if bomber_release(game, index).is_some() {
            continue;
        }
        let launch = vec2(
            shell.launch.x.to_num::<f32>(),
            shell.launch.y.to_num::<f32>(),
        );
        let (to, _) = payload_contact(game, sprites, index);
        // Indirect building fire is Bastion fire. Its sim launch stays at
        // the stable footprint center; presentation advances that point to
        // the authored barrel mouth.
        let from = shell_visual_origin(launch, to, shell.shooter, shell.kind);
        // Fog rule: own and allied shells draw throughout their flight;
        // a hostile shell appears only while its current local segment
        // crosses visible ground. Nothing anchors a trail at a fogged
        // muzzle and pinpoints the hidden artillery.
        let mine = !game.state.hostile(game.presentation.human, shell.player);
        let flat_seen = |k: f32| sees(from.lerp(to, k));
        // Reconstruct flight length the way the launch computed it, so
        // the shell lands exactly when the sim resolves the hit.
        let total = payload_flight_ticks(game, index);
        let elapsed = crate::audio_timeline::projectile_elapsed_ticks(now, shell.arrival, total);
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
            let width = game.presentation.camera.zoom * if missile { 0.078_125 } else { 0.13 };
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
                line_between(
                    exhaust,
                    back,
                    width * 0.60,
                    Color::from_rgba(182, 83, 35, 180),
                );
                let core = back - direction * game.presentation.camera.zoom * 0.08 * motor;
                line_between(
                    core,
                    back,
                    width * 0.30,
                    Color::from_rgba(246, 199, 116, 255),
                );
                if (exhaust - tail).dot(direction) > 0.0 {
                    line_between(
                        tail,
                        exhaust,
                        width * 0.70,
                        Color::from_rgba(112, 103, 90, 85),
                    );
                }
            }
            line_between(
                back,
                nose,
                width
                    + if missile {
                        game.presentation.camera.zoom * 0.035
                    } else {
                        2.0
                    },
                Color::from_rgba(12, 13, 17, 255),
            );
            let shoulder = nose - direction * length * 0.18;
            line_between(back, shoulder, width, Color::from_rgba(151, 146, 134, 255));
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
            let width = scale * if bastion_shell { 0.085 } else { 0.10 };
            let length = scale * if bastion_shell { 0.34 } else { 0.30 };
            let back = shell_at - direction * length * 0.5;
            let nose = shell_at + direction * length * 0.5;
            let shoulder = nose - direction * length * 0.34;
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
            line_between(
                back,
                shoulder,
                width + game.presentation.camera.zoom * 0.025,
                Color::from_rgba(14, 15, 18, 255),
            );
            line_between(back, shoulder, width, Color::from_rgba(123, 128, 127, 255));
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
        // A small shadow on the flat path makes the shell's low lift
        // legible.
        fill_circle(flat, radius * 0.7, Color::new(0.03, 0.03, 0.04, 0.35));
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
            line_between(
                tail_start,
                tail_end,
                radius * 0.56,
                Color::new(0.52, 0.18, 0.09, 0.92),
            );
            line_between(
                warm_start,
                tail_end,
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
        fill_circle(shell_at, radius * 1.35, Color::new(0.96, 0.42, 0.12, 0.16));
        let body_start = shell_at - travel * radius * 0.80;
        let body_end = shell_at + travel * radius * 0.36;
        line_between(
            body_start,
            body_end,
            radius * 1.28,
            Color::new(0.10, 0.09, 0.09, 1.0),
        );
        let nose = body_end;
        fill_circle(nose, radius * 0.54, Color::new(1.0, 0.82, 0.48, 1.0));
    }
    for fx in &game.presentation.fx {
        let impact_at = match fx.kind {
            EffectKind::Impact {
                at,
                from,
                surface,
                payload,
                ..
            } => Some(impact_contact(game, sprites, surface, from, at, payload)),
            _ => None,
        };
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
            EffectKind::Collapse { at, .. } => sees(at),
            EffectKind::Impact { at, .. } => sees(impact_at.unwrap_or(at)),
            EffectKind::Puff { at } => sees(at),
            // Falling fragments and airframes apply fog at their moving positions.
            EffectKind::Falling { .. } => true,
            EffectKind::Burst { at, .. } => sees(at),
            EffectKind::Debris { at, .. } => sees(at),
            // Own-order acknowledgments always show: each marks the
            // player's own click or a target the player already knows.
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
                surface,
                completed_tick,
                ..
            } => {
                use crate::game::ShotStyle;
                let contact = match surface {
                    Some(crate::game::HitSurface::Unit(hit)) => {
                        unit_contact(sprites, hit, from, hit.center)
                    }
                    _ => strike_contact(game, sprites, surface, from, to, style),
                }
                .filter(|&contact| game.presentation.all_seeing() || sees(contact))
                .unwrap_or(to);
                let a = game.presentation.camera.to_screen(from);
                let b = game.presentation.camera.to_screen(contact);
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
                    if let Some(family) = super::impacts::Family::direct(style) {
                        let arrival = match style {
                            ShotStyle::Rail => 0.,
                            ShotStyle::ForgeSpot => 0.12,
                            _ => 0.144,
                        };
                        super::impacts::draw(
                            super::impacts::Contact {
                                family,
                                recipient: super::impacts::Recipient::of(surface),
                                at: b,
                                direction: Vec2::ZERO,
                                radius: splash.unwrap_or(0.55),
                                age: age - arrival,
                                seed: super::impacts::seed(to, completed_tick),
                            },
                            game.presentation.camera.zoom,
                        );
                        continue;
                    }
                    if style == ShotStyle::Contact {
                        let normal = vec2(0.8, -0.6);
                        let tangent = vec2(-normal.y, normal.x);
                        let zoom = game.presentation.camera.zoom;
                        for side in [-1., 1.] {
                            let origin = b + tangent * side * zoom * 0.045;
                            let end = origin
                                + (normal * side + tangent * 0.35) * zoom * (0.06 + impact * 0.12);
                            line_between(
                                origin,
                                end,
                                (zoom * 0.035).max(1.),
                                Color::new(0.91, 0.69, 0.40, fade),
                            );
                        }
                        if impact < 0.4 {
                            fill_circle(
                                b,
                                zoom * 0.045 * (1. - impact),
                                Color::new(1., 0.88, 0.64, fade),
                            );
                        }
                        continue;
                    }
                    let seed = (to.x * 31.7 + to.y * 17.3).abs();
                    for i in 0..3 {
                        let angle = seed + i as f32 * 2.1;
                        let reach = game.presentation.camera.zoom * (0.08 + impact * 0.12);
                        let tip = b + vec2(angle.cos(), angle.sin()) * reach;
                        line_between(b, tip, 1.5, Color::new(0.95, 0.67, 0.34, 1.0 - impact));
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
                    ShotStyle::Kinetic { .. } | ShotStyle::Mortar => {
                        let heavy = !matches!(style, ShotStyle::Kinetic { heavy: false });
                        let (travel, impact) = direct_phases(style, age);
                        let direction = (b - a).normalize_or_zero();
                        let zoom = game.presentation.camera.zoom;
                        let round = a.lerp(b, travel);
                        let length = zoom * if heavy { 0.19 } else { 0.09 };
                        let tail = round - direction * length.min(round.distance(a));
                        let alpha = if style == ShotStyle::Mortar {
                            (1. - (age - 0.144).max(0.) / 0.05).clamp(0., 1.)
                        } else {
                            1. - impact
                        };
                        line_between(
                            tail,
                            round,
                            (zoom * if heavy { 0.065 } else { 0.03 }).max(1.0),
                            Color::new(0.91, 0.79, 0.57, alpha),
                        );
                        if style == ShotStyle::Mortar {
                            super::impacts::draw(
                                super::impacts::Contact {
                                    family: super::impacts::Family::Mortar,
                                    recipient: super::impacts::Recipient::of(surface),
                                    at: b,
                                    direction,
                                    radius: splash.unwrap_or(0.9),
                                    age: age - 0.144,
                                    seed: super::impacts::seed(to, completed_tick),
                                },
                                zoom,
                            );
                        } else if impact > 0.0 {
                            let normal = vec2(-direction.y, direction.x);
                            for side in [-1.0, 0.0, 1.0] {
                                let spread = (-direction + normal * side * 1.4).normalize_or_zero();
                                let reach =
                                    zoom * (0.05 + impact * if heavy { 0.25 } else { 0.16 });
                                let start = b + spread * reach * 0.55;
                                let end = b + spread * reach;
                                line_between(start, end, 1.0, Color::new(0.84, 0.66, 0.41, alpha));
                            }
                        }
                    }
                    ShotStyle::ForgeSpot => {
                        let (travel, _) = direct_phases(style, age);
                        let round = a.lerp(b, travel);
                        let round_alpha = (1. - (age - 0.12).max(0.) / 0.05).clamp(0., 1.);
                        fill_circle(round, 3.8, Color::new(1.0, 0.38, 0.10, 0.20 * round_alpha));
                        fill_circle(round, 2.3, Color::new(0.42, 0.19, 0.09, round_alpha));
                        fill_circle(round, 1.35, Color::new(1.0, 0.85, 0.52, round_alpha));
                        super::impacts::draw(
                            super::impacts::Contact {
                                family: super::impacts::Family::Orb,
                                recipient: super::impacts::Recipient::of(surface),
                                at: b,
                                direction: b - a,
                                radius: splash.unwrap_or(0.55),
                                age: age - 0.12,
                                seed: super::impacts::seed(to, completed_tick),
                            },
                            game.presentation.camera.zoom,
                        );
                    }
                    ShotStyle::Rail => {
                        super::impacts::draw(
                            super::impacts::Contact {
                                family: super::impacts::Family::Rail,
                                recipient: super::impacts::Recipient::of(surface),
                                at: b,
                                direction: b - a,
                                radius: 0.75,
                                age,
                                seed: super::impacts::seed(to, completed_tick),
                            },
                            game.presentation.camera.zoom,
                        );
                        line_between(
                            a,
                            b,
                            game.presentation.camera.zoom * 0.10 * fade.max(0.25),
                            Color::new(0.70, 0.76, 0.80, 0.18 * fade * fade),
                        );
                        line_between(
                            a,
                            b,
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
                            fill_circle(at, 3.4, Color::new(0.98, 0.43, 0.12, 0.18));
                            fill_circle(at, 2.0, Color::new(0.33, 0.24, 0.13, 1.0));
                            fill_circle(at, 1.2, Color::new(1.0, 0.84, 0.48, 1.0));
                        }
                        let seed = (to.x * 31.7 + to.y * 17.3).abs();
                        for i in 0..3 {
                            let angle = seed + i as f32 * 2.1;
                            let reach = impact * game.presentation.camera.zoom * 0.28;
                            let puff = b + vec2(angle.cos(), angle.sin()) * reach;
                            fill_circle(
                                puff,
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
                            source: Some(sprites.unit_action(oxide_sim::UnitKind::Sapper, 2)),
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
                        oxide_sim::UnitKind::Sapper
                            .stats()
                            .demolition
                            .map_or(0.0, |demolition| demolition.blast_radius.to_num::<f32>()),
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
                from,
                surface,
                completed_tick,
            } => {
                let contact = impact_at.unwrap_or(at);
                super::impacts::draw(
                    super::impacts::Contact {
                        family: super::impacts::Family::payload(payload),
                        recipient: super::impacts::Recipient::of(surface),
                        at: game.presentation.camera.to_screen(contact),
                        direction: if game.presentation.all_seeing() || sees(from) {
                            contact - from
                        } else {
                            Vec2::ZERO
                        },
                        radius,
                        age: fx
                            .age_at(game.state.current_tick(), game.presentation.tick_fraction()),
                        seed: super::impacts::seed(at, completed_tick),
                    },
                    game.presentation.camera.zoom,
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

/// Radar blips, drawn above the fog like pings: contacts without identity
/// from the Array's outer ring.
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
        // A hollow diamond in no faction's shape or color.
        let pts = [
            vec2(center.x, center.y - r),
            vec2(center.x + r, center.y),
            vec2(center.x, center.y + r),
            vec2(center.x - r, center.y),
        ];
        for i in 0..4 {
            let a = pts[i];
            let b = pts[(i + 1) % 4];
            line_between(a, b, 2.0, BONE_FAINT);
        }
        fill_circle(center, 2.0, BONE_FAINT);
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
            line_between(from, to, thickness, color);
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
    let segments = numeric::to_usize((std::f32::consts::TAU * radius / 4.0).ceil()).clamp(48, 240);
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
    let (width, height) = kind.size();
    let size = vec2(width as f32, height as f32);
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
    if building.player != game.presentation.human || !building.built() || building.hp == 0 {
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
    let (width, height) = building.kind.size();
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
    if let Some((kind, anchor)) = crate::input::placement_preview_anchor(game, input) {
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
        let steps = numeric::to_usize(depth.ceil());
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
                line_between(from, to, 1.2 * scale, color);
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
        .map(|building| (building.anchor, building.kind.size()));
    for bracket in support_brackets::junctions(footprints, game.presentation.camera.zoom, scale) {
        let origin = game
            .presentation
            .camera
            .to_screen(vec2(bracket.corner.x as f32, bracket.corner.y as f32));
        for [from, to] in bracket.segments(origin) {
            line_between(from, to, scale, color);
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
            let a = (band * count + point).fit::<u16>();
            let b = a + count.fit::<u16>();
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
            fill_circle(center, radius, dead_zone_fill(indicator.color));
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
        let EffectKind::Ping { at, kind, queued } = fx.kind else {
            continue;
        };
        let center = game.presentation.camera.to_screen(at);
        let progress = (fx.age / 0.5).clamp(0.0, 1.0);
        // Reduced motion draws a still ring instead of a collapsing one;
        // the verb color still says what was ordered.
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
        stroke_circle(center, radius, 2.5, color);
        if queued {
            draw_queued_mark(center, 5.0, color);
        }
    }
}

/// The selected own building's rally flag, above the fog for the same
/// reason as pings.
pub(crate) fn draw_rally_marker(game: &crate::game::Scene<'_>) {
    // A selected producer draws a line to its rally as well as the flag.
    // Own producers only, like the flag below: an inspected foreign
    // building must not reveal its rally.
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
            building.anchor.x as f32 + building.kind.size().0 as f32 * 0.5,
            building.anchor.y as f32 + building.kind.size().1 as f32 * 0.5,
        ));
        let b = game
            .presentation
            .camera
            .to_screen(vec2(rally.x as f32 + 0.5, rally.y as f32 + 0.5));
        line_between(a, b, 1.5, Color::new(0.91, 0.89, 0.85, 0.35));
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

/// A ring that fills around a resting finger until its long-press
/// order fires, so the hold reads as progress rather than a stall.
pub(crate) fn draw_long_press_ring(input: &InputState) {
    if let Some((at, progress)) = crate::input::long_press_progress(input) {
        draw_press_ring(at, progress);
    }
}

/// The same ring for a finger saving a control group. It draws after
/// the HUD and minimap, which would otherwise cover it.
pub(crate) fn draw_group_press_ring(input: &InputState) {
    if let Some((at, progress)) = crate::input::group_press_progress(input) {
        draw_press_ring(at, progress);
    }
}

fn draw_press_ring(at: Vec2, progress: f32) {
    let s = ui_scale();
    let radius = 32.0 * s;
    let thickness = 3.5 * s;
    stroke_circle(at, radius, 1.5 * s, Color::new(0.9, 0.88, 0.84, 0.3));
    crate::render::prim::stroke_arc(at, radius, thickness, progress * 360.0, BONE);
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
    // Live preview starts only once release would commit a box-select.
    draw_selection_rect(
        game,
        origin,
        now,
        feedback == crate::input::DragFeedback::Selection,
    );
}

/// The two-finger selection box, while a touch pair draws one; its unit
/// preview starts once the rest has claimed the box.
pub(crate) fn draw_touch_box(game: &crate::game::Scene<'_>, input: &InputState) {
    if let Some((a, b)) = crate::input::touch_box(input) {
        draw_selection_rect(game, a, b, true);
    }
}

/// A selection rectangle between two screen corners. With `preview`,
/// the own units a release would select are ringed.
fn draw_selection_rect(game: &crate::game::Scene<'_>, corner: Vec2, other: Vec2, preview: bool) {
    let lo = corner.min(other);
    let size = (corner - other).abs();
    draw_rectangle_lines(lo.x, lo.y, size.x, size.y, 1.5, BONE);
    draw_rectangle(
        lo.x,
        lo.y,
        size.x,
        size.y,
        Color::new(0.9, 0.88, 0.84, 0.08),
    );
    if !preview {
        return;
    }
    let a = game.presentation.camera.to_world(lo);
    let b = game.presentation.camera.to_world(lo + size);
    for unit in game.state.units() {
        if unit.player != game.presentation.human {
            continue;
        }
        let p = vec2(unit.pos.x.to_num::<f32>(), unit.pos.y.to_num::<f32>());
        if p.x >= a.x && p.x <= b.x && p.y >= a.y && p.y <= b.y {
            let screen = game.presentation.camera.to_screen(p);
            stroke_circle(
                screen,
                unit.kind.stats().radius.to_num::<f32>() * game.presentation.camera.zoom + 3.0,
                1.5,
                BONE_FAINT,
            );
        }
    }
}

#[cfg(test)]
mod tests;
