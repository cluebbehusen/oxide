//! Turning geometry for heading-first aircraft.
//!
//! A turn-limited flier moves on circles of one fixed radius, so every
//! steering choice is a question about where an arc of that circle would
//! carry the airframe. These helpers answer it in fixed point against the
//! map's flight envelope: the box an arc sweeps, whether that arc stays
//! inside the world, and whether a position and heading can still be flown
//! out of at all. Peaks are not modeled here; routes avoid them and the
//! per-step invalidation catches the rest.

use crate::map::Map;
use chassis::compass::dir;
use chassis::fx::{Fx, HALF, Vec2Fx};

/// The compass step that turns toward `+y` from `+x`.
pub(crate) const STEP_POS: u8 = 1;
/// The compass step that turns toward `-y` from `+x`.
pub(crate) const STEP_NEG: u8 = 255;
/// A half turn, in compass steps.
pub(crate) const HALF_TURN: u16 = 128;
/// A full circle, in compass steps.
pub(crate) const FULL_TURN: u16 = 256;

/// The opposite turn direction.
pub(crate) const fn reverse(step: u8) -> u8 {
    step.wrapping_neg()
}

/// Center of the circle an aircraft at `pos` on `heading` flies by holding
/// `step`.
pub(crate) fn turn_center(pos: Vec2Fx, heading: u8, step: u8, radius: Fx) -> Vec2Fx {
    let hv = dir(heading);
    let toward = if step == STEP_POS {
        Vec2Fx::new(-hv.y, hv.x)
    } else {
        Vec2Fx::new(hv.y, -hv.x)
    };
    pos + toward * radius
}

/// Axis-aligned bounds of the arc swept by holding `step` for `sweep`
/// compass steps: the start point, the end point, and every cardinal
/// extreme of the circle the sweep passes through.
pub(crate) fn arc_bounds(
    pos: Vec2Fx,
    heading: u8,
    step: u8,
    sweep: u16,
    radius: Fx,
) -> (Vec2Fx, Vec2Fx) {
    let sweep = sweep.min(FULL_TURN);
    let center = turn_center(pos, heading, step, radius);
    // The radial from the center to the aircraft trails a positive turn's
    // heading by a quarter and leads a negative one's.
    let radial = if step == STEP_POS {
        heading.wrapping_sub(64)
    } else {
        heading.wrapping_add(64)
    };
    let mut min = pos;
    let mut max = pos;
    let mut include = |p: Vec2Fx| {
        min = Vec2Fx::new(min.x.min(p.x), min.y.min(p.y));
        max = Vec2Fx::new(max.x.max(p.x), max.y.max(p.y));
    };
    let end = if step == STEP_POS {
        radial.wrapping_add(sweep as u8)
    } else {
        radial.wrapping_sub(sweep as u8)
    };
    include(center + dir(end) * radius);
    for cardinal in [0u8, 64, 128, 192] {
        let offset = if step == STEP_POS {
            cardinal.wrapping_sub(radial)
        } else {
            radial.wrapping_sub(cardinal)
        };
        if u16::from(offset) <= sweep {
            include(center + dir(cardinal) * radius);
        }
    }
    (min, max)
}

/// Where the aircraft is and which way it points after holding `step` for
/// `sweep` compass steps.
pub(crate) fn arc_end(pos: Vec2Fx, heading: u8, step: u8, sweep: u16, radius: Fx) -> (Vec2Fx, u8) {
    let center = turn_center(pos, heading, step, radius);
    let sweep = sweep.min(FULL_TURN) as u8;
    let (radial, end_heading) = if step == STEP_POS {
        (
            heading.wrapping_sub(64).wrapping_add(sweep),
            heading.wrapping_add(sweep),
        )
    } else {
        (
            heading.wrapping_add(64).wrapping_sub(sweep),
            heading.wrapping_sub(sweep),
        )
    };
    (center + dir(radial) * radius, end_heading)
}

/// Whether the arc swept by holding `step` for `sweep` steps stays inside
/// the flight envelope.
pub(crate) fn arc_fits(
    map: &Map,
    pos: Vec2Fx,
    heading: u8,
    step: u8,
    sweep: u16,
    radius: Fx,
) -> bool {
    let (min, max) = arc_bounds(pos, heading, step, sweep, radius);
    let max_x = Fx::from_num(map.width()) - HALF;
    let max_y = Fx::from_num(map.height()) - HALF;
    min.x >= HALF && min.y >= HALF && max.x <= max_x && max.y <= max_y
}

/// Whether an aircraft in this state can still be flown out of it: a half
/// turn in at least one direction stays inside the world. Flying parallel
/// to a wall passes; pointing at one from inside a turn radius does not,
/// and neither does diving into a corner from inside two.
pub(crate) fn escapable(map: &Map, pos: Vec2Fx, heading: u8, radius: Fx) -> bool {
    arc_fits(map, pos, heading, STEP_POS, HALF_TURN, radius)
        || arc_fits(map, pos, heading, STEP_NEG, HALF_TURN, radius)
}

/// How far an arc's bounds reach beyond the flight envelope, summed over
/// the four walls; zero when it fits.
fn violation(map: &Map, bounds: (Vec2Fx, Vec2Fx)) -> Fx {
    let (min, max) = bounds;
    let max_x = Fx::from_num(map.width()) - HALF;
    let max_y = Fx::from_num(map.height()) - HALF;
    (HALF - min.x).max(Fx::ZERO)
        + (HALF - min.y).max(Fx::ZERO)
        + (max.x - max_x).max(Fx::ZERO)
        + (max.y - max_y).max(Fx::ZERO)
}

/// The turn direction whose longest fitting arc is longest: the full
/// circle, then a half turn, then a quarter, then an eighth. When nothing
/// fits, the airframe is already pressed into a wall or corner, and the
/// bank whose quarter turn overshoots the world least brings the nose back
/// inside soonest. Positive wins ties so mirrored seats orbit mirrored
/// circles.
pub(crate) fn safest_step(map: &Map, pos: Vec2Fx, heading: u8, radius: Fx) -> u8 {
    for sweep in [FULL_TURN, HALF_TURN, 64, 32] {
        for step in [STEP_POS, STEP_NEG] {
            if arc_fits(map, pos, heading, step, sweep, radius) {
                return step;
            }
        }
    }
    let positive = violation(map, arc_bounds(pos, heading, STEP_POS, 64, radius));
    let negative = violation(map, arc_bounds(pos, heading, STEP_NEG, 64, radius));
    if negative < positive {
        STEP_NEG
    } else {
        STEP_POS
    }
}

/// The shorter turn direction toward `d` and its length in compass steps,
/// or `None` when the nose already points along `d`. Dead astern is a half
/// turn the negative way.
pub(crate) fn turn_to(heading: u8, d: Vec2Fx) -> Option<(u8, u16)> {
    let hv = dir(heading);
    let cross = hv.x * d.y - hv.y * d.x;
    let dot = hv.x * d.x + hv.y * d.y;
    if cross == Fx::ZERO && dot >= Fx::ZERO {
        return None;
    }
    let step = if cross > Fx::ZERO { STEP_POS } else { STEP_NEG };
    let mut positive = cross > Fx::ZERO;
    let mut h = heading;
    let mut sweep = HALF_TURN;
    for k in 1..=HALF_TURN {
        h = h.wrapping_add(step);
        let v = dir(h);
        let c = v.x * d.y - v.y * d.x;
        let t = v.x * d.x + v.y * d.y;
        // The sign also flips through dead astern; only a crossing with
        // the target ahead is the ray itself.
        if t >= Fx::ZERO && (c == Fx::ZERO || (c > Fx::ZERO) != positive) {
            sweep = k;
            break;
        }
        positive = c > Fx::ZERO;
    }
    Some((step, sweep))
}

/// The compass step whose direction lies closest to `v`.
pub(crate) fn heading_of(v: Vec2Fx) -> u8 {
    let mut best = 0u8;
    let mut best_dot = dir(0).x * v.x + dir(0).y * v.y;
    for k in 1..=255u8 {
        let d = dir(k);
        let dot = d.x * v.x + d.y * v.y;
        if dot > best_dot {
            best = k;
            best_dot = dot;
        }
    }
    best
}

#[cfg(test)]
mod tests {
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
    fn heading_of_rounds_to_the_nearest_compass_step() {
        assert_eq!(heading_of(Vec2Fx::new(Fx::lit("3"), Fx::ZERO)), 0);
        assert_eq!(heading_of(Vec2Fx::new(Fx::ZERO, Fx::lit("-2"))), 192);
        assert_eq!(heading_of(Vec2Fx::new(Fx::lit("1"), Fx::lit("1"))), 32);
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
}
