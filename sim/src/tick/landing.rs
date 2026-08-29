//! Run-ins and landings for heading-first aircraft.
//!
//! A bomber attacks and lands the same way it flies: on committed arcs.
//! Where a straight line would carry the airframe into a state no turn
//! recovers from, the run is planned through an initial point so the final
//! leg runs parallel to a wall. Landing adds the ground to that picture: a
//! tile is landable only when such a leg exists and the parked heading can
//! still be flown out of, and touchdown belongs to the brain rather than to
//! the ring the steering accepts ordinary waypoints in.

use super::flight;
use super::route_for_position;
use crate::ids::UnitId;
use crate::state::State;
use crate::stats::{Domain, LANDING_RUN_IN_DISTANCES, RUN_IN_DISTANCES, UnitKind, UnitStats};
use chassis::fx::{Fx, HALF, Vec2Fx};
use chassis::grid::TilePos;

/// What a run-in ends in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RunIn {
    /// An attack pass: the leg must be escapable where the acceptance ring
    /// hands the target back to the bomber.
    Attack,
    /// A landing: the leg must be escapable at the tile center itself, and
    /// the final leg is flown raw so touchdown meets that center.
    Landing,
}

fn sky_open(state: &State, tile: TilePos) -> bool {
    state
        .map()
        .tile(tile)
        .is_none_or(|t| !t.terrain.blocks_air())
}

/// Whether the tile can be landed on straight from here, on whatever
/// bearing it lies: the nose must settle onto that line before the tile (a
/// turn of `sweep` steps covers about a quarter of a radius per sixteen
/// steps of arc), the arrival heading must be one the airframe can fly out
/// of, and the line must be open sky. Only a tile behind the beam, or one
/// too close to line up on, needs a run-in.
pub(crate) fn straight_in(
    state: &State,
    stats: &UnitStats,
    pos: Vec2Fx,
    heading: u8,
    target: TilePos,
) -> bool {
    let map = state.map();
    let radius = stats.turn_radius();
    let goal = target.center();
    let approach = goal - pos;
    let length = approach.length();
    let sweep = flight::turn_to(heading, approach).map_or(0, |(_, sweep)| sweep);
    let room = length * Fx::from_num(64) >= radius * Fx::from_num(64 + i64::from(sweep));
    length > Fx::ZERO
        && sweep <= 64
        && room
        && flight::escapable(map, goal, flight::heading_of(approach), radius)
        && !chassis::path::line_blocked(pos, goal, |t| sky_open(state, t))
}

/// How a landing tile is chosen around a point.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pick {
    /// The nearest clear landable tile, the point itself first: an
    /// explicit goal is honoured as closely as the ground allows.
    Nearest,
    /// A tile that can be flown straight in from here before any other,
    /// then the nearest: an aircraft choosing its own pad has no reason to
    /// fly a procedure turn for a tile one step behind it.
    StraightIn,
}

/// A run-in the router may pick: its bearing, initial point, and the
/// initial point's distance from the tile.
struct Candidate {
    run_in: u8,
    point: TilePos,
    distance: i64,
}

/// The fix a landing run-in is entered from: twice the initial-point
/// distance out along the same bearing.
fn entry_fix(goal: Vec2Fx, v: Vec2Fx, distance: i64) -> TilePos {
    TilePos::containing(goal - v * Fx::from_num(2 * distance))
}

/// Where a run-in on bearing `v` must still be escapable: the acceptance
/// ring for an attack pass, the tile center for a landing.
fn arrival(mode: RunIn, goal: Vec2Fx, v: Vec2Fx, accept: Fx) -> Vec2Fx {
    match mode {
        RunIn::Attack => goal - v * accept,
        RunIn::Landing => goal,
    }
}

/// The initial points a run onto `target` may start from, one per bearing
/// whose final leg is escapable on arrival, each the farthest open point on
/// its bearing. Attack passes use the four cardinals; a landing may also
/// come in on a diagonal. A landing's final leg must also be clear sky, so
/// the raw leg to the center can never bend.
fn initial_points<'a>(
    state: &'a State,
    stats: &'a UnitStats,
    target: TilePos,
    mode: RunIn,
) -> impl Iterator<Item = (u8, TilePos, i64)> + 'a {
    let map = state.map();
    let radius = stats.turn_radius();
    let accept = stats.turn_acceptance();
    let goal = target.center();
    let bearings: &[u8] = match mode {
        RunIn::Attack => &[0, 64, 128, 192],
        RunIn::Landing => &[0, 32, 64, 96, 128, 160, 192, 224],
    };
    bearings.iter().copied().filter_map(move |run_in| {
        let v = chassis::compass::dir(run_in);
        if !flight::escapable(map, arrival(mode, goal, v, accept), run_in, radius) {
            return None;
        }
        let distances: &[i64] = match mode {
            RunIn::Attack => &RUN_IN_DISTANCES,
            RunIn::Landing => &LANDING_RUN_IN_DISTANCES,
        };
        distances.iter().copied().find_map(|distance| {
            let point = TilePos::containing(goal - v * Fx::from_num(distance));
            let open = match mode {
                RunIn::Attack => state.passable_for(Domain::Air, point),
                // A landing enters its run-in from a fix as far again
                // behind the initial point, so the leg is flown lined up
                // instead of joined from whatever heading reached it; the
                // whole line must be open sky.
                RunIn::Landing => {
                    let entry = entry_fix(goal, v, distance);
                    // Both fixes sit strictly inside the flight envelope
                    // and on a heading the airframe can fly out of: a fix
                    // on the boundary line is reachable only by flying
                    // pinned along the wall.
                    let flyable = |fix: TilePos| {
                        let c = fix.center();
                        state.passable_for(Domain::Air, fix)
                            && c.x > HALF
                            && c.y > HALF
                            && c.x < Fx::from_num(map.width()) - HALF
                            && c.y < Fx::from_num(map.height()) - HALF
                            && flight::escapable(map, c, run_in, radius)
                    };
                    flyable(point)
                        && flyable(entry)
                        && !chassis::path::line_blocked(entry.center(), goal, |t| {
                            sky_open(state, t)
                        })
                }
            };
            open.then_some((run_in, point, distance))
        })
    })
}

/// The route a heading-first aircraft flies onto `target`. A straight line
/// serves for an attack pass when the airframe can still be flown out of
/// the state it reaches at the edge of its acceptance ring, and for a
/// landing when it is already on final: close, aligned, and able to fly
/// out of the parked heading. Otherwise the run is planned through an
/// initial point so the final leg runs parallel to a wall: a corner target
/// is attacked or landed on along one of its walls instead of by a dive
/// the turn radius cannot recover from. Candidates are ranked by the turn
/// needed to head for the initial point, then by length, then by run-in
/// bearing relative to the current heading, so mirrored seats plan
/// mirrored runs. An attack pass falls back to the straight line when no
/// run fits; a landing has no fallback, because touching down there would
/// park an airframe that can never take off again.
pub(crate) fn run_in_route(
    state: &State,
    stats: &UnitStats,
    kind: UnitKind,
    pos: Vec2Fx,
    heading: u8,
    target: TilePos,
    mode: RunIn,
) -> Option<Vec<TilePos>> {
    let map = state.map();
    let radius = stats.turn_radius();
    let accept = stats.turn_acceptance();
    let goal = target.center();
    let approach = goal - pos;
    let length = approach.length();
    match mode {
        RunIn::Attack => {
            if length <= accept {
                return route_for_position(state, kind, pos, target);
            }
            let along = approach / length;
            if flight::escapable(
                map,
                goal - along * accept,
                flight::heading_of(along),
                radius,
            ) {
                return route_for_position(state, kind, pos, target);
            }
        }
        RunIn::Landing => {
            if straight_in(state, stats, pos, heading, target) {
                return Some(vec![target]);
            }
        }
    }
    let mut best: Option<((u16, Fx, u8), Candidate)> = None;
    for (run_in, point, distance) in initial_points(state, stats, target, mode) {
        let leg = point.center() - pos;
        if leg == Vec2Fx::ZERO {
            continue;
        }
        // An attack pass reaches its initial point on whatever heading the
        // leg gives it; a landing lines up on the run-in bearing there.
        let arrival_heading = match mode {
            RunIn::Attack => flight::heading_of(leg),
            RunIn::Landing => run_in,
        };
        if !flight::escapable(map, point.center(), arrival_heading, radius) {
            continue;
        }
        let turn = flight::turn_to(heading, leg).map_or(0, |(_, sweep)| sweep);
        // A landing also pays for the procedure turn it would need to line
        // up at the point, so a run flown straight in wins over one that
        // must come about first.
        let lining_up = match mode {
            RunIn::Attack => 0,
            RunIn::Landing => {
                flight::turn_to(flight::heading_of(leg), chassis::compass::dir(run_in))
                    .map_or(0, |(_, sweep)| sweep)
            }
        };
        let key = (
            turn + lining_up,
            leg.length() + Fx::from_num(distance),
            run_in.wrapping_sub(heading),
        );
        if best.as_ref().is_none_or(|(held, _)| key < *held) {
            best = Some((
                key,
                Candidate {
                    run_in,
                    point,
                    distance,
                },
            ));
        }
    }
    let Some((
        _,
        Candidate {
            run_in,
            point,
            distance,
        },
    )) = best
    else {
        return match mode {
            RunIn::Attack => route_for_position(state, kind, pos, target),
            RunIn::Landing => None,
        };
    };
    match mode {
        RunIn::Attack => {
            let mut waypoints = route_for_position(state, kind, pos, point)?;
            waypoints.extend(route_for_position(state, kind, point.center(), target)?);
            Some(waypoints)
        }
        RunIn::Landing => {
            let entry = entry_fix(goal, chassis::compass::dir(run_in), distance);
            let mut waypoints = route_for_position(state, kind, pos, entry)?;
            waypoints.push(point);
            waypoints.push(target);
            Some(waypoints)
        }
    }
}

/// Whether an aircraft of this kind can ever set down on `tile`: open
/// ground, and at least one run-in whose parked heading it could fly out
/// of again.
pub(crate) fn landable(state: &State, stats: &UnitStats, tile: TilePos) -> bool {
    state.passable_for(Domain::Ground, tile)
        && initial_points(state, stats, tile, RunIn::Landing)
            .next()
            .is_some()
}

/// Whether `tile` is landable and no other ground body is standing on it.
/// Whether `tile` is landable and a body resting at `at` on it would touch
/// no other ground body: nothing on the tile, and nothing within the two
/// bodies' combined radius of the resting point, since parked bodies are
/// immovable and an overlap between two of them would never resolve.
pub(crate) fn landing_clear(
    state: &State,
    stats: &UnitStats,
    me: UnitId,
    tile: TilePos,
    at: Vec2Fx,
) -> bool {
    landable(state, stats, tile)
        && !state.units().iter().any(|u| {
            let clearance = stats.radius + u.kind.stats().radius;
            u.id != me
                && u.hp > 0
                && u.domain() == Domain::Ground
                && (u.tile() == tile || u.pos.dist_sq(at) < clearance * clearance)
        })
}

/// The nearest clear landing tile to `around`, searched ring by ring and,
/// within a ring, by how little the aircraft would have to turn to head
/// for it. Bearings are taken relative to the current heading, so a
/// mirrored aircraft on a mirrored map picks the mirrored tile.
#[allow(clippy::too_many_arguments)]
pub(crate) fn nearest_landable(
    state: &State,
    stats: &UnitStats,
    me: UnitId,
    around: TilePos,
    pos: Vec2Fx,
    heading: u8,
    radius: i32,
    exclude: Option<TilePos>,
    pick: Pick,
) -> Option<TilePos> {
    let passes: &[bool] = match pick {
        Pick::Nearest => &[false],
        Pick::StraightIn => &[true, false],
    };
    for &straight in passes {
        for ring in 0..=radius {
            let mut best: Option<((u8, u8, i32, i32), TilePos)> = None;
            for dy in -ring..=ring {
                for dx in -ring..=ring {
                    if dx.abs().max(dy.abs()) != ring {
                        continue;
                    }
                    let tile = around.offset(dx, dy);
                    if exclude == Some(tile)
                        || !landing_clear(state, stats, me, tile, tile.center())
                        || (straight && !straight_in(state, stats, pos, heading, tile))
                    {
                        continue;
                    }
                    let bearing = flight::heading_of(tile.center() - pos).wrapping_sub(heading);
                    let key = (bearing.min(bearing.wrapping_neg()), bearing, tile.y, tile.x);
                    if best.as_ref().is_none_or(|(held, _)| key < *held) {
                        best = Some((key, tile));
                    }
                }
            }
            if let Some((_, tile)) = best {
                return Some(tile);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::Faction;
    use crate::command::Command;
    use crate::ids::PlayerId;
    use crate::scenario::{PlayerSpec, Scenario, UnitSpec};
    use crate::stats::UnitKind;
    use crate::{PlayerCommand, State};
    use chassis::fx::Fx;
    use chassis::grid::TilePos;

    fn arena() -> State {
        let mut rows = vec!["########################".to_string()];
        for _ in 0..14 {
            rows.push("#......................#".to_string());
        }
        rows.push("########################".to_string());
        rows[1] = "#1.....................#".to_string();
        rows[13] = "#....................2.#".to_string();
        Scenario {
            name: "mirror".into(),
            seed: 3,
            map: rows,
            players: vec![
                PlayerSpec {
                    name: "Ferrous".into(),
                    faction: Faction::Ferrous,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
                PlayerSpec {
                    name: "Cupric".into(),
                    faction: Faction::Cupric,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
            ],
            units: vec![
                UnitSpec {
                    player: 0,
                    kind: UnitKind::Condor,
                    x: 6,
                    y: 5,
                },
                UnitSpec {
                    player: 0,
                    kind: UnitKind::Condor,
                    x: 17,
                    y: 10,
                },
            ],
            buildings: Vec::new(),
            meta: None,
        }
        .build()
        .expect("the mirror arena builds")
    }

    /// Two aircraft placed as half-turn images of each other, given
    /// half-turn images of the same landing, must fly half-turn images of
    /// the same approach and park at the same moment: every choice on the
    /// way down is taken in a heading-relative frame.
    #[test]
    fn a_landing_mirrors_exactly_under_a_map_half_turn() {
        let mut state = arena();
        let (width, height) = (Fx::from_num(24), Fx::from_num(16));
        let a = state.units[0].id;
        let b = state.units[1].id;
        state.units[0].heading = 20;
        state.units[1].heading = 20u8.wrapping_add(128);
        let mirror = |p: chassis::fx::Vec2Fx| chassis::fx::Vec2Fx::new(width - p.x, height - p.y);
        assert_eq!(
            state.units[1].pos,
            mirror(state.units[0].pos),
            "premise: mirrored starts"
        );
        state.tick(&[
            PlayerCommand {
                player: PlayerId(0),
                command: Command::Move {
                    units: vec![a],
                    goal: TilePos::new(14, 7),
                    queue: false,
                },
            },
            PlayerCommand {
                player: PlayerId(0),
                command: Command::Move {
                    units: vec![b],
                    goal: TilePos::new(23 - 14, 15 - 7),
                    queue: false,
                },
            },
        ]);
        let mut landed_at = None;
        for _ in 0..1_500 {
            state.tick(&[]);
            let (ua, ub) = (state.unit(a).unwrap(), state.unit(b).unwrap());
            assert_eq!(
                ub.pos,
                mirror(ua.pos),
                "positions diverged at tick {}",
                state.tick
            );
            assert_eq!(
                ub.heading,
                ua.heading.wrapping_add(128),
                "headings diverged"
            );
            assert_eq!(ub.landed, ua.landed, "one landed before the other");
            if ua.landed {
                landed_at = Some(state.tick);
                break;
            }
        }
        assert!(landed_at.is_some(), "neither aircraft landed");
    }
}
