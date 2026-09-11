//! Role and allegiance cues at strategic camera scales.

use macroquad::prelude::*;
use oxide_sim::{Unit, UnitKind};

pub(crate) fn marker_alpha(zoom: f32) -> f32 {
    ((14.0 - zoom) / 4.0).clamp(0.0, 1.0)
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
    let (lo, hi) = game.camera.world_rect();
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
            let center = game.camera.to_screen(pos);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn marker_transition_has_a_continuous_bounded_fade() {
        assert_eq!(marker_alpha(32.0), 0.0);
        assert_eq!(marker_alpha(12.0), 0.5);
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
