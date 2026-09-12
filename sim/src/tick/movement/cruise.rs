//! Heading-first travel for aircraft that can stop and hover.

use chassis::compass::dir;
use chassis::fx::{Fx, Vec2Fx};
use chassis::grid::TilePos;

use crate::map::Map;
use crate::state::Unit;

fn clear_segment(map: &Map, from: Vec2Fx, to: Vec2Fx) -> bool {
    let open = |tile| {
        map.tile(tile)
            .is_some_and(|cell| !cell.terrain.blocks_air())
    };
    map.clamp_to_envelope(to) == to
        && open(TilePos::containing(to))
        && !chassis::path::line_blocked(from, to, open)
}

pub(super) fn advance(unit: &mut Unit, map: &Map) {
    let stats = unit.kind.stats();
    let rate = unit.kind.cruise_turn_rate();
    let radius = stats.speed * Fx::lit("40.75") / Fx::from_num(rate);
    loop {
        let Some(path) = &mut unit.path else {
            return;
        };
        let Some(&waypoint) = path.waypoints.get(path.next as usize) else {
            unit.path = None;
            return;
        };
        let center = waypoint.center();
        if let Some(&next) = path.waypoints.get(path.next as usize + 1)
            && unit.pos.dist_sq(center) <= radius * radius
            && clear_segment(map, unit.pos, next.center())
        {
            path.next += 1;
            continue;
        }
        let offset = center - unit.pos;
        let distance = offset.length();
        if distance == Fx::ZERO {
            path.next += 1;
            continue;
        }
        let aligned = super::steer_bearing(&mut unit.heading, offset, rate);
        // Below cruise radius, slowing tightens the arc enough to converge
        // on a nearby waypoint instead of orbiting it indefinitely.
        let speed = stats.speed.min(distance * Fx::from_num(rate) / 64);
        let next = if aligned && distance <= stats.speed {
            center
        } else {
            unit.pos + dir(unit.heading) * speed
        };
        if clear_segment(map, unit.pos, next) {
            unit.pos = next;
        } else {
            // Hover while the nose turns clear. Replan from the actual
            // position if the arc has carried us behind a Peak.
            unit.path = None;
        }
        return;
    }
}
