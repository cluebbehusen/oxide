//! Worker mechanisms, attached to the chassis and the visible work surface.
use super::*;
use crate::presentation_animation::{ExcavatorTool, UnitAnimationState, UnitWorkState, WorkTarget};

fn world(pos: chassis::fx::Vec2Fx) -> Vec2 {
    vec2(pos.x.to_num(), pos.y.to_num())
}

fn rotate(v: Vec2, angle: f32) -> Vec2 {
    vec2(
        v.x * angle.cos() - v.y * angle.sin(),
        v.x * angle.sin() + v.y * angle.cos(),
    )
}

fn contact(
    game: &Scene<'_>,
    sprites: &Sprites,
    target: WorkTarget,
    from: Vec2,
    alpha: f32,
) -> Option<Vec2> {
    match target {
        WorkTarget::Building(id) => {
            let b = game.state.building(id)?;
            if !game.presentation.all_seeing()
                && !b.tiles().any(|tile| game.my_vision().visible(tile))
            {
                return None;
            }
            entities::building_contact(
                game,
                sprites,
                crate::game::BuildingHit::capture(game.state, b),
                from,
                world(
                    game.state
                        .contact_surface(b)
                        .closest(chassis::fx::Vec2Fx::new(
                            chassis::fx::Fx::from_num(from.x),
                            chassis::fx::Fx::from_num(from.y),
                        )),
                ),
            )
        }
        WorkTarget::Scrap(tile) | WorkTarget::Wreck(tile) => {
            if !game.presentation.all_seeing() && !game.my_vision().visible(tile) {
                return None;
            }
            let wreck = matches!(target, WorkTarget::Wreck(_));
            let source = if wreck {
                sprites.wreck_pile()
            } else {
                sprites.scrap(
                    game.state.map().scrap_at(tile),
                    oxide_sim::stats::SCRAP_NODE_AMOUNT,
                )
            };
            let origin = vec2(tile.x as f32, tile.y as f32);
            let flip = wreck
                && (tile
                    .x
                    .wrapping_mul(31)
                    .wrapping_add(tile.y.wrapping_mul(17)) as usize)
                    % 5
                    < 2;
            let to_local = |p: Vec2| {
                let p = p - origin;
                if flip { vec2(1.0 - p.x, p.y) } else { p }
            };
            let point = sprites.sprite_contact(
                source,
                to_local(from),
                Vec2::splat(0.5),
                Vec2::ZERO,
                Vec2::ONE,
            )?;
            Some(
                origin
                    + if flip {
                        vec2(1.0 - point.x, point.y)
                    } else {
                        point
                    },
            )
        }
        WorkTarget::Unit(id) => {
            let patient = game.state.unit(id)?;
            if !crate::strategic_markers::visible(game, patient) {
                return None;
            }
            let pose = super::unit_body_pose(game, sprites, patient, alpha);
            let (center, rotation, size, source) =
                (pose.center, pose.body_rotation, pose.size, pose.source);
            let point = sprites.sprite_contact(
                source,
                rotate(from - center, -rotation),
                Vec2::ZERO,
                Vec2::splat(-size * 0.5),
                Vec2::splat(size),
            )?;
            Some(center + rotate(point, rotation))
        }
    }
}

fn contact_surface(
    game: &Scene<'_>,
    sprites: &Sprites,
    target: WorkTarget,
    from: Vec2,
    alpha: f32,
) -> Option<Vec2> {
    contact(game, sprites, target, from, alpha)
        .map(|point| game.presentation.camera.to_screen(point))
}

fn forward_work_point(point: Vec2, root: Vec2, forward: Vec2, scale: f32) -> Vec2 {
    let lateral = vec2(-forward.y, forward.x);
    let delta = point - root;
    let ahead = delta.dot(forward).clamp(20. * scale, 30. * scale);
    let sideways = delta.dot(lateral).clamp(-12. * scale, 12. * scale);
    root + forward * ahead + lateral * sideways
}

fn smooth_step(value: f32) -> f32 {
    let t = value.clamp(0., 1.);
    t * t * (3. - 2. * t)
}

fn arm_joints(target: Vec2, deployment: f32) -> [Vec2; 3] {
    let mount = vec2(86., 66.);
    let upper: f32 = 34.;
    let lower: f32 = 38.;
    let delta = target - mount;
    let distance = delta
        .length()
        .clamp((lower - upper).abs() + 0.5, upper + lower - 0.5);
    let direction = delta.normalize_or_zero();
    let along = (upper * upper - lower * lower + distance * distance) / (2. * distance);
    let height = (upper * upper - along * along).max(0.).sqrt();
    let elbow = mount + direction * along + vec2(-direction.y, direction.x) * height;
    let endpoint = mount + direction * distance;
    let upper_angle = (elbow - mount).to_angle();
    let lower_angle = (endpoint - elbow).to_angle();
    let turn = |from: f32, to: f32, phase: f32| {
        let delta = (to - from + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        from + delta * smooth_step(phase)
    };
    let shoulder = turn(std::f32::consts::FRAC_PI_2, upper_angle, deployment / 0.65);
    let elbow_fold = turn(
        -std::f32::consts::PI + 0.10,
        lower_angle - upper_angle,
        (deployment - 0.35) / 0.65,
    );
    let elbow = mount + Vec2::from_angle(shoulder) * upper;
    [
        mount,
        elbow,
        elbow + Vec2::from_angle(shoulder + elbow_fold) * lower,
    ]
}

/// The rigid links nest along the chassis side, inside the tread line.
pub(super) fn draw_welder(
    game: &Scene<'_>,
    sprites: &Sprites,
    unit: &oxide_sim::Unit,
    animation: UnitAnimationState,
    pose: (Vec2, f32, f32),
    alpha: f32,
) {
    if unit.kind != oxide_sim::UnitKind::Excavator {
        return;
    }
    let (center, rotation, size) = pose;
    let width = size / 128.;
    let lateral = rotate(Vec2::X, rotation);
    let from = game.presentation.draw_pos(unit.id, unit.pos, alpha)
        + lateral * super::unit_draw_scale(unit.kind) * 0.32;
    let arm = animation.welding_arm;
    let deployment = arm.map_or(0., |arm| arm.deployment);
    let target = arm.and_then(|arm| contact_surface(game, sprites, arm.target, from, alpha));
    let target_local = target.map_or(vec2(86., 13.), |point| {
        rotate(point - center, -rotation) / width + Vec2::splat(64.)
    });
    let local = |point: Vec2| center + rotate((point - Vec2::splat(64.)) * width, rotation);
    let joints = arm_joints(target_local, deployment).map(local);
    let [mount, elbow, tip] = joints;
    let outline = Color::from_rgba(20, 24, 24, 255);
    let iron = Color::from_rgba(79, 89, 88, 255);
    let steel = Color::from_rgba(142, 153, 145, 255);
    let plate = [
        vec2(81., 61.),
        vec2(91., 61.),
        vec2(91., 71.),
        vec2(81., 71.),
    ]
    .map(local);
    draw_triangle(plate[0], plate[1], plate[2], outline);
    draw_triangle(plate[0], plate[2], plate[3], outline);
    let plate_edge = [vec2(82., 62.), vec2(90., 62.)].map(local);
    draw_line(
        plate_edge[0].x,
        plate_edge[0].y,
        plate_edge[1].x,
        plate_edge[1].y,
        width,
        iron,
    );
    for (a, b, thickness, metal) in [(mount, elbow, 3., iron), (elbow, tip, 2., steel)] {
        draw_line(a.x, a.y, b.x, b.y, (thickness + 2.) * width, outline);
        draw_line(a.x, a.y, b.x, b.y, thickness * width, metal);
    }
    for point in [mount, elbow] {
        draw_circle(point.x, point.y, 3. * width, outline);
        draw_circle(point.x, point.y, 1.5 * width, steel);
    }
    let neck = tip + (elbow - tip).normalize_or_zero() * 7. * width;
    draw_line(neck.x, neck.y, tip.x, tip.y, 4. * width, outline);
    draw_line(
        neck.x,
        neck.y,
        tip.x,
        tip.y,
        2. * width,
        Color::from_rgba(161, 112, 68, 255),
    );
    if arm.is_some_and(|arm| arm.active)
        && deployment >= 0.98
        && target.is_some_and(|point| point.distance(tip) < 3. * width)
    {
        let cycle = match animation.work.excavator_tool() {
            ExcavatorTool::WeldingArm { cycle } => cycle,
            ExcavatorTool::Drum { .. } | ExcavatorTool::Stowed => 0.,
        };
        let pulse = if reduced_motion() {
            0.5
        } else {
            (cycle * 29.).sin().abs()
        };
        draw_circle(
            tip.x,
            tip.y,
            (1. + pulse) * width,
            Color::from_rgba(246, 214, 146, 255),
        );
    }
}

pub(super) fn draw(
    game: &Scene<'_>,
    sprites: &Sprites,
    unit: &oxide_sim::Unit,
    animation: UnitAnimationState,
    pose: (Vec2, f32, f32),
    alpha: f32,
) {
    if unit.kind == oxide_sim::UnitKind::Scuttler {
        draw_scuttler(game, sprites, unit, animation, pose, alpha);
        return;
    }
    let (center, rotation, size) = pose;
    let local = |x: f32, y: f32| center + rotate(vec2(x - 64., y - 64.) * (size / 128.), rotation);
    let from = game.presentation.draw_pos(unit.id, unit.pos, alpha);
    let surface = animation
        .work_target
        .and_then(|target| contact(game, sprites, target, from, alpha));
    let stripping_wreck = matches!(animation.work_target, Some(WorkTarget::Wreck(_)));
    let target = surface.map(|p| {
        let point = game.presentation.camera.to_screen(p);
        if stripping_wreck {
            let root = local(
                64.,
                if unit.kind == oxide_sim::UnitKind::Harvester {
                    48.
                } else {
                    53.
                },
            );
            forward_work_point(point, root, rotate(vec2(0., -1.), rotation), size / 128.)
        } else {
            point
        }
    });
    let (cycle, holding, unloading) = match animation.work {
        UnitWorkState::Harvesting { cycle, .. } | UnitWorkState::Salvaging { cycle, .. } => {
            (cycle, false, false)
        }
        UnitWorkState::Repairing { cycle, .. } | UnitWorkState::Constructing { cycle, .. } => {
            (cycle, true, false)
        }
        UnitWorkState::Unloading { progress, .. } => (progress, false, true),
        UnitWorkState::Idle => (0., false, false),
    };
    let reach = if holding {
        1.0
    } else if unloading {
        (cycle * 2.).min(1.)
    } else {
        (cycle / 0.3).min(1.) * ((1.0 - cycle) / 0.35).min(1.)
    };
    let contact = target.map(|p| {
        let root = local(64., 48.);
        let offset = p - root;
        let limit = size * (34. / 128.);
        let surface = root + offset.clamp_length_max(limit);
        let at = local(64., 25.).lerp(surface, reach);
        if !holding && !unloading && cycle > 0.65 {
            let return_y = if stripping_wreck { 43. } else { 67. };
            at.lerp(local(64., return_y), ((cycle - 0.65) / 0.35).clamp(0., 1.))
        } else {
            at
        }
    });
    let width = size / 128.;
    let outline = Color::from_rgba(20, 24, 24, 255);
    let iron = Color::from_rgba(79, 89, 88, 255);
    let steel = Color::from_rgba(142, 153, 145, 255);
    let edge = Color::from_rgba(189, 195, 174, 255);
    let forward = rotate(vec2(0., -1.), rotation);
    let lateral = rotate(Vec2::X, rotation);
    if unit.kind == oxide_sim::UnitKind::Harvester {
        for side in [-1., 1.] {
            let root = local(64. + side * 12., 48.);
            let grip = if holding {
                4.
            } else if cycle > 0.3 {
                6.
            } else {
                14.
            };
            let tip = contact.map_or_else(
                || local(64. + side * 14., 23.),
                |p| p + lateral * side * grip * width,
            );
            let tip = root + (tip - root).clamp_length_max(34. * width);
            let elbow = root.lerp(tip, 0.55) + lateral * side * 4. * width;
            for (a, b, metal, thickness) in [(root, elbow, iron, 6.), (elbow, tip, steel, 4.)] {
                draw_line(a.x, a.y, b.x, b.y, 8. * width, outline);
                draw_line(a.x, a.y, b.x, b.y, thickness * width, metal);
            }
            for p in [root, elbow] {
                draw_circle(p.x, p.y, 3. * width, outline);
                draw_circle(p.x, p.y, 1.6 * width, steel);
            }
            let jaw = |x: f32, y: f32| {
                tip + lateral * side * x * 0.65 * width - forward * (y * 0.65 + 2.) * width
            };
            let points = [
                jaw(-2., -6.),
                jaw(10., -3.),
                jaw(10., 8.),
                jaw(-5., 11.),
                jaw(-5., 6.),
                jaw(3., 4.),
            ];
            for [a, b, c] in [[0, 1, 5], [1, 2, 5], [2, 3, 4], [2, 4, 5]] {
                draw_triangle(points[a], points[b], points[c], outline);
            }
            let a = jaw(8., -2.);
            let b = jaw(8., 6.);
            let c = jaw(-3., 8.);
            draw_line(a.x, a.y, b.x, b.y, 3. * width, iron);
            draw_line(b.x, b.y, c.x, c.y, 3. * width, iron);
            let a = jaw(-2., -5.);
            let b = jaw(7., -2.);
            draw_line(a.x, a.y, b.x, b.y, width, edge);
        }
        if !holding
            && !unloading
            && (0.5..0.98).contains(&cycle)
            && let Some(p) = contact
        {
            draw_rectangle(
                p.x - 3. * width,
                p.y - 2. * width,
                6. * width,
                4. * width,
                Color::from_rgba(151, 100, 47, 255),
            );
        }
        if holding && let Some(p) = contact {
            let flicker = if reduced_motion() {
                0.5
            } else {
                (cycle * 29.).sin().abs()
            };
            draw_circle(
                p.x,
                p.y,
                (1.0 + flicker) * width,
                Color::from_rgba(246, 214, 146, 255),
            );
        }
    } else {
        let tender = unit.kind == oxide_sim::UnitKind::Tender;
        let root = if tender {
            local(81., 58.)
        } else {
            local(64., 53.)
        };
        let rest = if tender {
            local(76., 33.)
        } else {
            local(64., 28.)
        };
        let limit = if tender { 44. } else { 34. } * width;
        let drum = match animation.work.excavator_tool() {
            ExcavatorTool::Drum { cycle } => Some(cycle),
            ExcavatorTool::WeldingArm { .. } | ExcavatorTool::Stowed => None,
        };
        let roller_parked = !tender && drum.is_none();
        let tool_target = if roller_parked { None } else { target };
        let tip = if tender {
            tool_target.map_or(rest, |p| {
                let surface = root + (p - root).clamp_length_max(limit);
                rest.lerp(surface, if holding { 1. } else { reach })
            })
        } else {
            tool_target.map_or(rest, |p| forward_work_point(p, root, forward, width))
        };
        if tender {
            let elbow = root.lerp(tip, 0.55) + lateral * 7. * width;
            for (a, b, w, color) in [(root, elbow, 5., iron), (elbow, tip, 3., steel)] {
                draw_line(a.x, a.y, b.x, b.y, 9. * width, outline);
                draw_line(a.x, a.y, b.x, b.y, w * width, color);
            }
            for p in [root, elbow] {
                draw_circle(p.x, p.y, 3. * width, outline);
                draw_circle(p.x, p.y, 1.5 * width, steel);
            }
            let reel = local(68., 90.);
            let hose = local(91., 72.);
            for (a, b) in [(reel, hose), (hose, elbow)] {
                draw_line(a.x, a.y, b.x, b.y, 3. * width, outline);
                draw_line(a.x, a.y, b.x, b.y, width, iron);
            }
            let nozzle = tip - forward * 6. * width;
            draw_line(nozzle.x, nozzle.y, tip.x, tip.y, 6. * width, outline);
            draw_line(nozzle.x, nozzle.y, tip.x, tip.y, 2. * width, steel);
        } else {
            for side in [-1., 1.] {
                let base = local(64. + side * 21., 53.);
                let end = tip + lateral * side * 25. * width;
                draw_line(base.x, base.y, end.x, end.y, 7. * width, outline);
                draw_line(base.x, base.y, end.x, end.y, 3. * width, steel);
                draw_circle(base.x, base.y, 3. * width, outline);
                draw_circle(base.x, base.y, 1.5 * width, iron);
            }
            let a = tip - lateral * 30. * width;
            let b = tip + lateral * 30. * width;
            draw_line(a.x, a.y, b.x, b.y, 16. * width, outline);
            draw_line(a.x, a.y, b.x, b.y, 11. * width, iron);
            for side in [-1., 1.] {
                let cap = tip + lateral * side * 30. * width;
                let a = cap - forward * 6. * width;
                let b = cap + forward * 6. * width;
                draw_line(a.x, a.y, b.x, b.y, 5. * width, outline);
                draw_line(a.x, a.y, b.x, b.y, 2. * width, steel);
            }
            let phase = drum.filter(|_| !reduced_motion()).unwrap_or(0.);
            for column in 0..6 {
                for row in 0..3 {
                    let angle = (phase + row as f32 / 3. + (column % 2) as f32 / 6.)
                        * std::f32::consts::TAU;
                    let face = angle.cos();
                    if face <= 0. {
                        continue;
                    }
                    let p = tip
                        + lateral * (column as f32 - 2.5) * 9. * width
                        + forward * angle.sin() * 4. * width;
                    let half = lateral * 3.5 * width;
                    let point = p + forward * (4. + 5. * face) * width + lateral * 1.5 * width;
                    draw_triangle(p - half, p + half, point, outline);
                    let inset = forward * width;
                    draw_triangle(
                        p - half * 0.65 + inset,
                        p + half * 0.65 + inset,
                        point - forward * 1.5 * width,
                        steel,
                    );
                    let a = p - half * 0.65 + inset;
                    let b = point - forward * 1.5 * width;
                    draw_line(a.x, a.y, b.x, b.y, width, edge);
                }
            }
        }
        if holding && !roller_parked && target.is_some_and(|p| p.distance(tip) < 5. * width) {
            let pulse = if reduced_motion() {
                0.5
            } else {
                (cycle * 29.).sin().abs()
            };
            draw_circle(
                tip.x,
                tip.y,
                (1. + pulse) * width,
                Color::from_rgba(246, 214, 146, 255),
            );
        }
    }
}

fn draw_scuttler(
    game: &Scene<'_>,
    sprites: &Sprites,
    unit: &oxide_sim::Unit,
    animation: UnitAnimationState,
    pose: (Vec2, f32, f32),
    alpha: f32,
) {
    use crate::presentation_animation::{AttackPhase, WeaponCycle};
    let (center, rotation, size) = pose;
    let width = size / 128.;
    let local = |point: Vec2| center + rotate((point - Vec2::splat(64.)) * width, rotation);
    let from = game.presentation.draw_pos(unit.id, unit.pos, alpha);
    let target = if let oxide_sim::Order::Attack { target, .. } = unit.order {
        game.state
            .attack_view(unit.player, target)
            .and_then(|view| view.entity)
            .and_then(|target| {
                contact(
                    game,
                    sprites,
                    match target {
                        oxide_sim::Target::Unit(id) => WorkTarget::Unit(id),
                        oxide_sim::Target::Building(id) => WorkTarget::Building(id),
                    },
                    from,
                    alpha,
                )
            })
            .filter(|point| point.distance(from) < 0.85)
    } else {
        None
    };
    let target = target.or_else(|| {
        animation.attack?;
        game.presentation.fx.iter().rev().find_map(|effect| {
            if let crate::game::EffectKind::DirectShot {
                attacker: Some(attacker),
                surface,
                from,
                to,
                style: crate::game::ShotStyle::Contact,
                ..
            } = effect.kind
                && attacker == unit.id
            {
                entities::strike_contact(
                    game,
                    sprites,
                    surface,
                    from,
                    to,
                    crate::game::ShotStyle::Contact,
                )
            } else {
                None
            }
        })
    });
    let surface = target.map(|point| {
        rotate(
            game.presentation.camera.to_screen(point) - center,
            -rotation,
        ) / width
            + Vec2::splat(64.)
    });
    let (gap, extension) = match animation.attack {
        Some(AttackPhase::Report { .. }) => (1.5, 1.),
        Some(AttackPhase::Recover { progress, .. }) => (
            1.5 + 12.5 * smooth_step(progress),
            1. - smooth_step(progress),
        ),
        None => match animation.weapons[0] {
            WeaponCycle::Preparing { progress } => {
                (14. + 6. * smooth_step((progress - 0.5) * 2.), 0.)
            }
            _ => (14., 0.),
        },
    };
    let aim = surface.unwrap_or(vec2(64., 18.));
    let aim = vec2(aim.x.clamp(48., 80.), aim.y.clamp(-12., 25.));
    let tip_center = vec2(64., 18.).lerp(aim, extension);
    let outline = Color::from_rgba(20, 24, 24, 255);
    let iron = Color::from_rgba(79, 89, 88, 255);
    let steel = Color::from_rgba(166, 177, 164, 255);
    for side in [-1., 1.] {
        let root = local(vec2(64. + side * 13., 47.));
        let elbow_local = vec2(64. + side * (25. + (gap - 14.) * 0.25), 31.).lerp(
            vec2(64. + side * 25., f32::midpoint(tip_center.y, 47.)),
            extension,
        );
        let elbow = local(elbow_local);
        let tip_local = tip_center + vec2(side * gap, 0.);
        let tip = local(tip_local);
        draw_line(root.x, root.y, elbow.x, elbow.y, 9. * width, outline);
        draw_line(root.x, root.y, elbow.x, elbow.y, 5. * width, iron);
        let heel = local(elbow_local + vec2(-side * 8., 5.));
        let shoulder = local(tip_local + vec2(side * 9., 2.));
        draw_triangle(elbow, shoulder, tip, outline);
        draw_triangle(elbow, tip, heel, outline);
        draw_line(heel.x, heel.y, tip.x, tip.y, 2. * width, steel);
        let edge = local(tip_local + vec2(side * 6., 1.));
        draw_line(tip.x, tip.y, edge.x, edge.y, 2. * width, steel);
        draw_circle(root.x, root.y, 3. * width, outline);
        draw_circle(root.x, root.y, 1.5 * width, steel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_wrecks_never_pull_the_tool_behind_its_mount() {
        for bearing in [0., 0.7, 1.5, 3., 4.5] {
            let forward = Vec2::from_angle(bearing);
            let root = vec2(120., 80.);
            for offset in [Vec2::ZERO, -forward * 40., vec2(100., -100.)] {
                let tip = forward_work_point(root + offset, root, forward, 1.);
                assert!((tip - root).dot(forward) >= 19.99);
                assert!(tip.distance(root) < 34.);
            }
        }
    }

    #[test]
    fn stowed_welder_nests_against_the_body_without_entering_the_treads() {
        for target in [vec2(64., 0.), vec2(110., 10.), vec2(45., 35.)] {
            let joints = arm_joints(target, 0.);
            assert_eq!(joints[0], vec2(86., 66.));
            for point in joints {
                assert!((83. ..91.).contains(&point.x));
                assert!((59. ..102.).contains(&point.y));
            }
        }
    }

    #[test]
    fn welder_links_keep_their_length_while_folding_to_the_side() {
        let target = vec2(64., 19.);
        for step in 0..=20 {
            let [mount, elbow, tip] = arm_joints(target, step as f32 / 20.);
            assert!((elbow.distance(mount) - 34.).abs() < 0.001);
            assert!((tip.distance(elbow) - 38.).abs() < 0.001);
        }
        let [mount, elbow, tip] = arm_joints(target, 1.);
        assert_eq!(mount, vec2(86., 66.));
        assert!(elbow.x > 90., "elbow should clear the side of the roller");
        assert!(tip.distance(target) < 0.001);
    }
}
