//! Role and allegiance cues at strategic camera scales.

use macroquad::prelude::*;
use oxide_sim::{Unit, UnitKind};

thread_local! {
    static PREFS: std::cell::Cell<crate::config::MarkerPrefs> =
        std::cell::Cell::new(crate::config::MarkerPrefs::default());
}

pub(crate) fn set_prefs(prefs: crate::config::MarkerPrefs) {
    PREFS.set(prefs.clamped());
}

pub(crate) fn prefs() -> crate::config::MarkerPrefs {
    PREFS.get()
}

pub(crate) fn marker_alpha(zoom: f32) -> f32 {
    let p = prefs();
    transition(zoom, p.start, p.end)
}

fn marker_camera(viewport: Vec2, scale: f32) -> Camera2D {
    Camera2D {
        target: viewport / (2.0 * scale),
        zoom: vec2(2.0 * scale / viewport.x, 2.0 * scale / viewport.y),
        ..Default::default()
    }
}

fn transition(zoom: f32, start: f32, end: f32) -> f32 {
    ((start - zoom) / (start - end)).clamp(0.0, 1.0)
}

pub(crate) fn replaces_units(zoom: f32) -> bool {
    marker_alpha(zoom) >= 1.0
}

#[derive(Clone, Copy)]
enum Role {
    Worker,
    Gun,
    Siege,
    AntiAir,
    Scout,
    Support,
    Transport,
    Demolition,
}

fn role(kind: UnitKind) -> Role {
    use UnitKind::*;
    match kind {
        Harvester | Excavator => Role::Worker,
        Sentinel | Lancer | Buzzard | Darter | Warden | Breaker => Role::Gun,
        Bombard | Condor | Moth | Avalanche => Role::Siege,
        Flakhound | Stinger | Wisp | Talon | Shrike | Sylph => Role::AntiAir,
        Kestrel | Gnat => Role::Scout,
        Tender => Role::Support,
        Skyhook => Role::Transport,
        Scuttler | Sapper => Role::Demolition,
    }
}

pub(crate) fn visible(game: &crate::game::Game, unit: &Unit) -> bool {
    unit.player == game.human || game.all_seeing() || game.my_vision().visible(unit.tile())
}

pub fn draw_markers(game: &crate::game::Game, alpha: f32) {
    let opacity = marker_alpha(game.camera.zoom);
    if opacity <= 0.0 {
        return;
    }
    let scale = prefs().scale;
    let (lo, hi) = game.camera.world_rect();
    let viewport = game.camera.viewport();
    push_camera_state();
    set_camera(&marker_camera(viewport, scale));
    // Draw all backplates before any glyph so adjacent markers cannot erase roles.
    for backplates in [true, false] {
        for unit in game
            .state
            .units()
            .iter()
            .filter(|u| !game.selection.units.contains(&u.id))
            .chain(
                game.state
                    .units()
                    .iter()
                    .filter(|u| game.selection.units.contains(&u.id)),
            )
        {
            if !visible(game, unit) {
                continue;
            }
            let pos = game.draw_pos(unit.id, unit.pos, alpha);
            if pos.x < lo.x - 2.0 || pos.y < lo.y - 2.0 || pos.x > hi.x + 2.0 || pos.y > hi.y + 2.0
            {
                continue;
            }
            let screen = game.camera.to_screen(pos);
            let center = screen / scale;
            let tint = crate::render::seat_identity_color(game, unit.player);
            let color = Color { a: opacity, ..tint };
            let fill = Color::new(0.065, 0.065, 0.075, opacity);
            let ink = Color::new(0.91, 0.89, 0.83, opacity);
            let selected = game.selection.units.contains(&unit.id);
            let air = unit.domain() == oxide_sim::stats::Domain::Air;
            if backplates {
                if air {
                    draw_poly(center.x, center.y, 4, 10.0, 0.0, fill);
                    draw_poly_lines(center.x, center.y, 4, 10.0, 0.0, 1.6, color);
                } else {
                    draw_rectangle(center.x - 7.0, center.y - 7.0, 14.0, 14.0, fill);
                    draw_rectangle_lines(center.x - 7.0, center.y - 7.0, 14.0, 14.0, 1.6, color);
                }
                continue;
            }
            let line = |a: (f32, f32), b: (f32, f32)| {
                draw_line(
                    center.x + a.0,
                    center.y + a.1,
                    center.x + b.0,
                    center.y + b.1,
                    1.4,
                    ink,
                )
            };
            match role(unit.kind) {
                Role::Worker => {
                    line((-3.0, -3.0), (-3.0, 3.0));
                    line((3.0, -3.0), (3.0, 3.0));
                    line((-3.0, 3.0), (3.0, 3.0));
                }
                Role::Gun => {
                    draw_circle_lines(center.x, center.y + 1.0, 2.4, 1.3, ink);
                    line((0.0, 0.0), (0.0, -4.5));
                }
                Role::Siege => {
                    line((-3.5, 2.0), (0.0, -2.0));
                    line((0.0, -2.0), (3.5, 2.0));
                    line((-3.5, 4.0), (3.5, 4.0));
                }
                Role::AntiAir => {
                    line((-3.5, 1.5), (0.0, -2.0));
                    line((0.0, -2.0), (3.5, 1.5));
                    line((0.0, -2.0), (0.0, 4.0));
                }
                Role::Scout => {
                    draw_circle_lines(center.x, center.y, 3.0, 1.3, ink);
                    draw_circle(center.x, center.y, 1.0, ink);
                }
                Role::Support => {
                    line((-3.5, 0.0), (3.5, 0.0));
                    line((0.0, -3.5), (0.0, 3.5));
                }
                Role::Transport => {
                    line((-3.0, -2.0), (-3.0, 2.0));
                    line((3.0, -2.0), (3.0, 2.0));
                    line((-3.0, 2.0), (3.0, 2.0));
                    line((-3.0, -2.0), (3.0, -2.0));
                }
                Role::Demolition => {
                    line((-3.0, -3.0), (3.0, 3.0));
                    line((-3.0, 3.0), (3.0, -3.0));
                }
            }
            if game.state.hostile(game.human, unit.player) {
                draw_triangle(
                    center + vec2(-3.0, 8.0),
                    center + vec2(3.0, 8.0),
                    center + vec2(0.0, 11.0),
                    color,
                );
            } else if unit.player != game.human {
                draw_line(
                    center.x - 3.0,
                    center.y + 9.0,
                    center.x + 3.0,
                    center.y + 9.0,
                    1.5,
                    ink,
                );
            }
            if selected {
                draw_circle_lines(center.x, center.y, 12.0, 1.3, ink);
            }
            if unit.hp < unit.kind.stats().max_hp {
                draw_rectangle(center.x - 7.0, center.y - 12.0, 14.0, 2.0, fill);
                draw_rectangle(
                    center.x - 7.0,
                    center.y - 12.0,
                    14.0 * unit.hp as f32 / unit.kind.stats().max_hp as f32,
                    2.0,
                    ink,
                );
            }
        }
    }
    pop_camera_state();
}

pub(crate) fn building_visible(game: &crate::game::Game, b: &oxide_sim::Building) -> bool {
    b.player == game.human
        || game.all_seeing()
        || (b.tiles().any(|t| game.my_vision().visible(t))
            && game.state.building_apparent(game.human, b))
}

pub(crate) fn draw_buildings(game: &crate::game::Game) {
    let alpha = marker_alpha(game.camera.zoom);
    if alpha <= 0.0 {
        return;
    }
    let scale = prefs().scale;
    let (lo, hi) = game.camera.world_rect();
    let mut markers = Vec::new();
    for b in game
        .state
        .buildings()
        .iter()
        .filter(|b| building_visible(game, b))
    {
        markers.push((
            b.kind,
            b.player,
            b.anchor,
            b.built,
            false,
            game.selection.buildings.contains(&b.id),
            1.0,
        ));
    }
    if !game.all_seeing() {
        for g in game.my_vision().ghosts() {
            let observed = game
                .state
                .buildings_at(g.anchor)
                .any(|b| b.kind == g.kind && b.player == g.owner && building_visible(game, b));
            if observed {
                continue;
            }
            let age = game
                .last_seen
                .borrow()
                .get(&(g.anchor.x, g.anchor.y))
                .map_or(0.0, |stamp| game.fx_time() - stamp);
            markers.push((
                g.kind,
                g.owner,
                g.anchor,
                g.built,
                true,
                false,
                0.55 * (1.0 - crate::render::staleness_fade(age)),
            ));
        }
    }
    let viewport = game.camera.viewport();
    push_camera_state();
    set_camera(&marker_camera(viewport, scale));
    for (kind, owner, anchor, built, memory, selected, fade) in markers {
        let (w, h) = kind.base_stats().size;
        let world = vec2(
            anchor.x as f32 + w as f32 * 0.5,
            anchor.y as f32 + h as f32 * 0.5,
        );
        if world.x < lo.x - 3.0
            || world.y < lo.y - 3.0
            || world.x > hi.x + 3.0
            || world.y > hi.y + 3.0
        {
            continue;
        }
        let p = game.camera.to_screen(world) / scale;
        let opacity = alpha * fade;
        let identity = crate::render::seat_identity_color(game, owner);
        let edge = Color {
            a: opacity,
            ..identity
        };
        let ink = Color::new(0.91, 0.89, 0.83, opacity);
        let fill = Color::new(0.065, 0.065, 0.075, opacity * 0.94);
        draw_poly(p.x, p.y, 8, 10.0, 22.5, fill);
        draw_poly_lines(p.x, p.y, 8, 10.0, 22.5, 1.4, edge);
        building_glyph(kind, p, ink);
        if !built {
            draw_line(p.x - 6.0, p.y + 6.0, p.x + 6.0, p.y - 6.0, 1.3, ink);
        }
        if memory {
            draw_circle(p.x + 9.0, p.y - 9.0, 2.0, ink);
        }
        if selected {
            draw_poly_lines(p.x, p.y, 8, 13.0, 22.5, 1.4, ink);
        }
        if game.state.hostile(game.human, owner) {
            draw_triangle(
                p + vec2(-3.0, 11.0),
                p + vec2(3.0, 11.0),
                p + vec2(0.0, 14.0),
                edge,
            );
        } else if owner != game.human {
            draw_line(p.x - 3.0, p.y + 12.0, p.x + 3.0, p.y + 12.0, 1.4, ink);
        }
    }
    pop_camera_state();
}

fn building_glyph(kind: oxide_sim::BuildingKind, p: Vec2, ink: Color) {
    use oxide_sim::BuildingKind::*;
    let line = |a: (f32, f32), b: (f32, f32)| {
        draw_line(p.x + a.0, p.y + a.1, p.x + b.0, p.y + b.1, 1.35, ink)
    };
    match kind {
        Foundry | Fabricator | Crucible => {
            line((-4.0, 4.0), (4.0, 4.0));
            line((-4.0, 4.0), (-4.0, -2.0));
            line((-4.0, -2.0), (-1.0, 0.0));
            line((-1.0, 0.0), (2.0, -2.0));
            line((2.0, -2.0), (4.0, -2.0));
            line((4.0, -2.0), (4.0, 4.0));
            line((-4.0, -2.0), (-4.0, -5.0));
            if kind != Foundry {
                line((0.0, -3.0), (0.0, -5.0));
            }
            if kind == Crucible {
                line((4.0, -3.0), (4.0, -5.0));
            }
        }
        Airworks => {
            line((-5.0, 1.0), (5.0, 1.0));
            line((0.0, -5.0), (0.0, 5.0));
            line((-3.0, 4.0), (3.0, 4.0));
        }
        Turret => {
            draw_circle_lines(p.x, p.y + 1.0, 2.8, 1.3, ink);
            line((0.0, 0.0), (0.0, -5.0));
        }
        FlakTurret => {
            line((-4.0, 1.0), (0.0, -3.0));
            line((0.0, -3.0), (4.0, 1.0));
            line((0.0, -3.0), (0.0, 5.0));
        }
        Bastion => {
            line((-4.0, 1.0), (0.0, -3.0));
            line((0.0, -3.0), (4.0, 1.0));
            line((-4.0, 4.0), (4.0, 4.0));
        }
        Array => {
            draw_circle_lines(p.x, p.y, 4.0, 1.1, ink);
            line((0.0, 0.0), (3.0, -4.0));
            draw_circle(p.x, p.y, 1.4, ink);
        }
        RepairBay => {
            line((-4.0, 0.0), (4.0, 0.0));
            line((0.0, -4.0), (0.0, 4.0));
        }
        Reclaimer => {
            line((-4.0, -4.0), (4.0, -4.0));
            line((4.0, -4.0), (4.0, 0.0));
            line((4.0, 0.0), (0.0, 4.0));
            line((0.0, 4.0), (-4.0, 0.0));
            line((-4.0, 0.0), (-4.0, -4.0));
        }
        Extractor => {
            line((-4.0, -3.0), (0.0, 2.0));
            line((0.0, 2.0), (4.0, -3.0));
            line((0.0, 2.0), (0.0, 5.0));
        }
        Barricade => {
            for x in [-4.0, 0.0, 4.0] {
                line((x, -4.0), (x, 4.0));
            }
        }
        ScuttleCharge => {
            line((-3.0, -3.0), (3.0, 3.0));
            line((-3.0, 3.0), (3.0, -3.0));
        }
    }
}

pub(crate) fn extractor_frame_visible(
    game: &crate::game::Game,
    frame: chassis::grid::TilePos,
) -> bool {
    let known = game.all_seeing()
        || (0..2).any(|dy| (0..2).any(|dx| game.my_vision().explored(frame.offset(dx, dy))));
    let claimed = if game.all_seeing() {
        game.state
            .buildings()
            .iter()
            .any(|building| building.hp > 0 && building.anchor == frame)
    } else {
        game.state.extractor_frame_claim_known(game.human, frame)
    };
    known && !claimed
}

pub(crate) fn draw_extractor_frames(game: &crate::game::Game) {
    let prefs = prefs();
    let alpha = marker_alpha(game.camera.zoom);
    if alpha <= 0.0 {
        return;
    }
    let scale = prefs.scale;
    let (lo, hi) = game.camera.world_rect();
    push_camera_state();
    set_camera(&marker_camera(game.camera.viewport(), scale));
    for &frame in game.state.map().extractor_frames() {
        let pos = vec2(frame.x as f32 + 1.0, frame.y as f32 + 1.0);
        if pos.x < lo.x - 2.0
            || pos.y < lo.y - 2.0
            || pos.x > hi.x + 2.0
            || pos.y > hi.y + 2.0
            || !extractor_frame_visible(game, frame)
        {
            continue;
        }
        let p = game.camera.to_screen(pos) / scale;
        let ink = Color::new(0.83, 0.66, 0.36, alpha);
        draw_rectangle(
            p.x - 8.0,
            p.y - 8.0,
            16.0,
            16.0,
            Color::new(0.065, 0.065, 0.075, alpha * 0.9),
        );
        for (x, y) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            draw_line(
                p.x + x * 8.0,
                p.y + y * 8.0,
                p.x + x * 4.0,
                p.y + y * 8.0,
                1.4,
                ink,
            );
            draw_line(
                p.x + x * 8.0,
                p.y + y * 8.0,
                p.x + x * 8.0,
                p.y + y * 4.0,
                1.4,
                ink,
            );
        }
        building_glyph(oxide_sim::BuildingKind::Extractor, p, ink);
    }
    pop_camera_state();
}

pub(crate) fn known_resource(game: &crate::game::Game, pos: chassis::grid::TilePos) -> u32 {
    if game.all_seeing() || game.my_vision().visible(pos) {
        game.state
            .map()
            .tile(pos)
            .map_or(0, |t| t.scrap.saturating_add(t.wreck))
    } else {
        game.my_vision()
            .remembered_scrap(pos)
            .saturating_add(game.my_vision().remembered_wreck(pos))
    }
}
pub(crate) fn draw_resources(game: &crate::game::Game) {
    let p = prefs();
    let opacity = transition(
        game.camera.zoom,
        (p.start - 4.0).max(p.end - 1.0),
        p.end - 2.0,
    ) * 0.65;
    if opacity <= 0.0 {
        return;
    }
    let mut groups = std::collections::BTreeMap::<(i32, i32), (Vec2, u32, bool)>::new();
    let (lo, hi) = game.camera.world_rect();
    // World-anchored cells keep summaries stable while the camera pans.
    for y in ((lo.y.floor() as i32).div_euclid(4) * 4).max(0)
        ..(((hi.y.ceil() as i32).div_euclid(4) + 1) * 4).min(game.state.map().height())
    {
        for x in ((lo.x.floor() as i32).div_euclid(4) * 4).max(0)
            ..(((hi.x.ceil() as i32).div_euclid(4) + 1) * 4).min(game.state.map().width())
        {
            let pos = chassis::grid::TilePos::new(x, y);
            if known_resource(game, pos) == 0 {
                continue;
            }
            let group =
                groups
                    .entry((x.div_euclid(4), y.div_euclid(4)))
                    .or_insert((Vec2::ZERO, 0, true));
            group.0 += vec2(x as f32 + 0.5, y as f32 + 0.5);
            group.1 += 1;
            group.2 &= game.all_seeing() || game.my_vision().visible(pos);
        }
    }
    for (_, (sum, count, visible)) in groups {
        let p = game.camera.to_screen(sum / count as f32);
        let alpha = opacity * if visible { 1.0 } else { 0.4 };
        let scale = prefs().scale;
        let size = if count > 1 { 4.0 } else { 2.8 } * scale;
        draw_poly(
            p.x,
            p.y,
            4,
            size + 1.5,
            0.0,
            Color::new(0.04, 0.04, 0.05, alpha),
        );
        draw_poly_lines(
            p.x,
            p.y,
            4,
            size,
            0.0,
            1.3 * scale,
            Color::new(0.70, 0.53, 0.26, alpha),
        );
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extractor_frame_markers_share_exploration_and_claim_visibility() {
        use chassis::grid::TilePos;
        let scenario: oxide_sim::scenario::Scenario =
            serde_json::from_str(include_str!("../../scenarios/skirmish.json")).unwrap();
        let mut game =
            crate::game::Game::with_viewport(scenario.clone(), vec2(1280.0, 800.0)).unwrap();
        let near = TilePos::new(8, 4);
        let far = TilePos::new(30, 18);
        assert!(game.state.map().is_extractor_frame(near));
        assert!(game.state.map().is_extractor_frame(far));
        assert!(extractor_frame_visible(&game, near));
        assert!(!extractor_frame_visible(&game, far));
        game.overlay = true;
        assert!(extractor_frame_visible(&game, far));

        let mut claimed = scenario;
        for (player, anchor) in [(0, near), (1, far)] {
            claimed.buildings.push(oxide_sim::scenario::BuildingSpec {
                player,
                kind: oxide_sim::BuildingKind::Extractor,
                x: anchor.x,
                y: anchor.y,
            });
        }
        let mut game = crate::game::Game::with_viewport(claimed, vec2(1280.0, 800.0)).unwrap();
        assert!(!extractor_frame_visible(&game, near));
        assert!(!extractor_frame_visible(&game, far));
        game.overlay = true;
        assert!(!extractor_frame_visible(&game, near));
        assert!(!extractor_frame_visible(&game, far));
    }

    #[test]
    fn marker_scaling_preserves_the_screen_position_and_top_left_origin() {
        use macroquad::camera::Camera;
        let viewport = vec2(1280.0, 800.0);
        for scale in [0.75, 1.0, 1.5] {
            let camera = marker_camera(viewport, scale);
            let top_left = camera.matrix().transform_point3(vec3(0.0, 0.0, 0.0));
            assert!((top_left.x + 1.0).abs() < 0.0001);
            assert!((top_left.y - 1.0).abs() < 0.0001);
            let p = vec2(330.0, 140.0);
            let projected = camera.matrix().transform_point3((p / scale).extend(0.0));
            assert!((projected.x - (p.x / 640.0 - 1.0)).abs() < 0.0001);
            assert!((projected.y - (1.0 - p.y / 400.0)).abs() < 0.0001);
        }
    }
    #[test]
    fn enemy_mine_marker_stays_hidden_on_visible_ground() {
        let mut scenario: oxide_sim::scenario::Scenario =
            serde_json::from_str(include_str!("../../scenarios/skirmish.json")).unwrap();
        scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
            player: 1,
            kind: oxide_sim::BuildingKind::ScuttleCharge,
            x: 10,
            y: 4,
        });
        let game = crate::game::Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
        let mine = game
            .state
            .buildings()
            .iter()
            .find(|b| b.kind == oxide_sim::BuildingKind::ScuttleCharge)
            .unwrap();
        assert!(game.my_vision().visible(mine.anchor));
        assert!(!building_visible(&game, mine));
    }
    #[test]
    fn earlier_transition_reaches_full_opacity_before_the_old_transition_starts() {
        assert_eq!(transition(24.0, 24.0, 16.0), 0.0);
        assert_eq!(transition(20.0, 24.0, 16.0), 0.5);
        assert_eq!(transition(16.0, 24.0, 16.0), 1.0);
        assert_eq!(transition(16.0, 14.0, 10.0), 0.0);
    }
    #[test]
    fn markers_do_not_reveal_unseen_buildings_or_resources() {
        let scenario: oxide_sim::scenario::Scenario =
            serde_json::from_str(include_str!("../../scenarios/skirmish.json")).unwrap();
        let game = crate::game::Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
        for b in game.state.buildings() {
            assert_eq!(building_visible(&game, b), b.player == game.human);
        }
        let mut hidden_nodes = 0;
        for y in 0..game.state.map().height() {
            for x in 0..game.state.map().width() {
                let p = chassis::grid::TilePos::new(x, y);
                if !game.my_vision().explored(p) && game.state.map().tile(p).unwrap().scrap > 0 {
                    assert_eq!(known_resource(&game, p), 0);
                    hidden_nodes += 1;
                }
            }
        }
        assert!(hidden_nodes > 0);
    }
    #[test]
    fn marker_transition_has_a_continuous_bounded_fade() {
        assert_eq!(marker_alpha(32.0), 0.0);
        assert_eq!(marker_alpha(20.0), 0.5);
        assert_eq!(marker_alpha(8.0), 1.0);
    }

    #[test]
    fn strategic_markers_obey_player_sight_and_explicit_debug_visibility() {
        let source = include_str!("../../scenarios/skirmish.json");
        let scenario: oxide_sim::scenario::Scenario = serde_json::from_str(source).unwrap();
        let mut game =
            crate::game::Game::with_viewport(scenario.clone(), vec2(1280.0, 800.0)).unwrap();
        assert!(visible(&game, &game.state.units()[0]));
        assert!(!visible(&game, &game.state.units()[4]));
        game.overlay = true;
        assert!(visible(&game, &game.state.units()[4]));
        let mut nearby = scenario;
        nearby.units[4].x = 10;
        nearby.units[4].y = 9;
        let game = crate::game::Game::with_viewport(nearby, vec2(1280.0, 800.0)).unwrap();
        assert!(visible(&game, &game.state.units()[4]));
    }
}
