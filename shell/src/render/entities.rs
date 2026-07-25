//! Everything that lives on the ground: buildings, units, effects,
//! pings, range rings, radar blips, rally lines, breadcrumbs, the
//! placement ghost, and the drag rectangle.

use super::*;

/// The armed building follows the cursor as a translucent footprint,
/// green-lit where the sim would accept it — the tint and the command
/// share `State::can_place`, so what looks legal is legal.
pub(crate) fn draw_placement_ghost(game: &Game, sprites: &Sprites, input: &InputState) {
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

pub(crate) fn draw_buildings(game: &Game, sprites: &Sprites) {
    let zoom = game.camera.zoom;
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
            // A remembered hostile twin keeps its dark border (scaled
            // with the memory's fade): a translucent own-faction sprite
            // is also how the player's own construction sites draw, and
            // a memory must never masquerade as one of those.
            let twin_border = allegiance_cue(game, ghost.owner) == AllegianceCue::HostileTwin;
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
            if twin_border {
                draw_rectangle_lines(
                    screen.x,
                    screen.y,
                    w as f32 * zoom,
                    h as f32 * zoom,
                    3.0,
                    Color::new(0.05, 0.05, 0.07, 0.7 * fade),
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
        // A hostile building wearing the player's own colors (team maps
        // pit same-faction seats against each other) gets a dark border
        // — the buildings' face of the unit ring's luminance cue.
        if allegiance_cue(game, building.player) == AllegianceCue::HostileTwin {
            draw_rectangle_lines(
                screen.x,
                screen.y,
                dest.x,
                dest.y,
                3.0,
                Color::new(0.05, 0.05, 0.07, 0.7),
            );
        }
        // A rising site wears its scaffold: dense lattice early, sparse
        // once the hull carries the silhouette, gone at completion.
        // Progress-keyed, so reduced motion needs no special case.
        if !building.built {
            let ticks = building
                .kind
                .stats()
                .construction
                .map(|c| c.build_ticks)
                .unwrap_or(1);
            let frac = (building.progress as f32 / ticks as f32).clamp(0.0, 1.0);
            draw_texture_ex(
                sprites.texture(),
                screen.x,
                screen.y,
                Color::new(1.0, 1.0, 1.0, 0.85 - 0.45 * frac),
                DrawTextureParams {
                    dest_size: Some(dest),
                    source: Some(sprites.scaffold(frac < 0.5)),
                    ..Default::default()
                },
            );
        }
        if building.built && building.kind == oxide_sim::BuildingKind::Foundry {
            // The melt pool breathes: a soft faction-tinted pulse.
            // Decorative motion — reduced motion holds it at its
            // midpoint (which also keeps the shot suite's pinned
            // backdrop from drifting with the wall clock).
            let pulse = if reduced_motion() {
                0.5
            } else {
                ((get_time() * 2.6 + f64::from(building.id.0)).sin() * 0.5 + 0.5) as f32
            };
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

pub(crate) fn draw_units(game: &Game, sprites: &Sprites, alpha: f32) {
    // Two passes: ground bodies first, then everything airborne above
    // them — each flyer casts an offset shadow so altitude reads even
    // when nothing overlaps.
    draw_unit_pass(game, sprites, alpha, oxide_sim::stats::Domain::Ground);
    draw_unit_pass(game, sprites, alpha, oxide_sim::stats::Domain::Air);
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
            EffectKind::Debris { at, .. } => sees(at),
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

pub(crate) fn draw_range_rings(game: &Game, input: &InputState) {
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

pub(crate) fn draw_drag_rect(game: &Game, input: &InputState) {
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
