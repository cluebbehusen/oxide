use super::*;
use crate::map::Map;

fn open_map(width: usize, height: usize) -> Map {
    let rows: Vec<String> = (0..height).map(|_| ".".repeat(width)).collect();
    Map::parse(&rows).expect("an open field parses").0
}

const R: Fx = Fx::lit("2");

#[test]
fn a_quarter_turn_sweeps_one_radius_ahead_and_one_to_the_side() {
    let pos = Vec2Fx::new(Fx::lit("10"), Fx::lit("10"));
    let (min, max) = arc_bounds(pos, 0, STEP_POS, 64, R);
    assert_eq!(min, pos);
    assert_eq!(max, Vec2Fx::new(Fx::lit("12"), Fx::lit("12")));
    let (end, heading) = arc_end(pos, 0, STEP_POS, 64, R);
    assert_eq!(end, max);
    assert_eq!(heading, 64);
    let (min, max) = arc_bounds(pos, 0, STEP_NEG, 64, R);
    assert_eq!(min, Vec2Fx::new(Fx::lit("10"), Fx::lit("8")));
    assert_eq!(max, Vec2Fx::new(Fx::lit("12"), Fx::lit("10")));
}

#[test]
fn a_full_circle_spans_two_radii_regardless_of_heading() {
    let pos = Vec2Fx::new(Fx::lit("10"), Fx::lit("10"));
    for heading in [0u8, 37, 64, 100, 192, 250] {
        for step in [STEP_POS, STEP_NEG] {
            let (min, max) = arc_bounds(pos, heading, step, FULL_TURN, R);
            let span = max - min;
            assert!(
                span.x > Fx::lit("3.99") && span.x < Fx::lit("4.01"),
                "heading {heading} step {step}: x span {}",
                span.x
            );
            assert!(span.y > Fx::lit("3.99") && span.y < Fx::lit("4.01"));
        }
    }
}

#[test]
fn parallel_flight_beside_a_wall_is_escapable_but_pointing_at_it_is_not() {
    let map = open_map(40, 40);
    let beside_east_wall = Vec2Fx::new(Fx::lit("38.5"), Fx::lit("20"));
    assert!(
        escapable(&map, beside_east_wall, 192, R),
        "northbound beside the wall"
    );
    assert!(
        !escapable(&map, beside_east_wall, 0, R),
        "eastbound into the wall"
    );
    let two_radii_out = Vec2Fx::new(Fx::lit("35"), Fx::lit("20"));
    assert!(
        escapable(&map, two_radii_out, 0, R),
        "eastbound with room to turn"
    );
}

#[test]
fn a_corner_dive_is_unescapable_inside_two_radii_of_both_walls() {
    let map = open_map(40, 40);
    let near = Vec2Fx::new(Fx::lit("36.5"), Fx::lit("36.5"));
    assert!(
        !escapable(&map, near, 32, R),
        "south-east dive three tiles out"
    );
    let far = Vec2Fx::new(Fx::lit("34"), Fx::lit("34"));
    assert!(
        escapable(&map, far, 32, R),
        "south-east dive with room to reverse"
    );
}

#[test]
fn turn_to_reports_the_short_way_and_half_turns_for_dead_astern() {
    assert_eq!(turn_to(0, Vec2Fx::new(Fx::lit("5"), Fx::ZERO)), None);
    assert_eq!(
        turn_to(0, Vec2Fx::new(Fx::ZERO, Fx::lit("5"))),
        Some((STEP_POS, 64))
    );
    assert_eq!(
        turn_to(0, Vec2Fx::new(Fx::ZERO, Fx::lit("-5"))),
        Some((STEP_NEG, 64))
    );
    assert_eq!(
        turn_to(0, Vec2Fx::new(Fx::lit("-5"), Fx::ZERO)),
        Some((STEP_NEG, 128))
    );
    assert_eq!(
        turn_to(192, Vec2Fx::new(Fx::lit("1"), Fx::lit("1"))),
        Some((STEP_POS, 96))
    );
}

#[test]
fn a_half_turn_exactly_reverses_every_compass_direction() {
    // Bearing-relative choices mirror under a map half-turn only
    // because the compass table itself is exactly antisymmetric.
    for k in 0..=255u8 {
        let d = dir(k);
        let opposite = dir(k.wrapping_add(128));
        assert_eq!(opposite, Vec2Fx::new(-d.x, -d.y), "step {k}");
    }
}

#[test]
fn a_pinned_corner_aircraft_banks_toward_the_nearer_inward_heading() {
    let map = open_map(40, 40);
    let corner = Vec2Fx::new(Fx::lit("39.5"), Fx::lit("0.5"));
    // Pointing north-north-east: the negative bank reaches a westward
    // component after a quarter turn, the positive one only after more.
    assert_eq!(safest_step(&map, corner, 208, R), STEP_NEG);
    // East-north-east: the positive bank reaches a southward component first.
    assert_eq!(safest_step(&map, corner, 240, R), STEP_POS);
}

#[test]
fn the_safest_step_prefers_the_circle_that_fits() {
    let map = open_map(40, 40);
    let beside_east_wall = Vec2Fx::new(Fx::lit("38.5"), Fx::lit("20"));
    assert_eq!(safest_step(&map, beside_east_wall, 192, R), STEP_NEG);
    let beside_west_wall = Vec2Fx::new(Fx::lit("1.5"), Fx::lit("20"));
    assert_eq!(safest_step(&map, beside_west_wall, 192, R), STEP_POS);
    let open = Vec2Fx::new(Fx::lit("20"), Fx::lit("20"));
    assert_eq!(safest_step(&map, open, 192, R), STEP_POS);
}

#[test]
fn a_corner_is_escapable_with_the_nose_toward_open_ground() {
    let map = open_map(40, 40);
    let corner = Vec2Fx::new(Fx::lit("38.5"), Fx::lit("38.5"));
    assert!(
        escapable(&map, corner, 160, R),
        "north-west out of the south-east corner"
    );
    assert!(!escapable(&map, corner, 32, R), "south-east into it");
    assert!(
        !escapable(&map, corner, 64, R),
        "south along the east wall from the corner"
    );
}
