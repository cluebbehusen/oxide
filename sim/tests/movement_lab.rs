//! The movement lab: instrumented, `#[ignore]`d diagnostics that put
//! integer numbers on movement feel — the 0.12 pathfinding campaign's
//! before/after instrument (head-on grind, overtake drag, crossing,
//! group strandings, parked bulldozing, pursuit kiting, bulk-attack
//! surround utilization). No assertions on the contested numbers:
//! the lab measures, the behavior suites pin. Run explicitly:
//!
//! `cargo test -p oxide-sim --test movement_lab -- --ignored --nocapture`

mod common;

use chassis::fx::Fx;
use chassis::grid::TilePos;
use common::{cmd, open_arena as lane_arena, open_arena_with as lane_arena_with};
use oxide_sim::{Command, Event, Order, State, UnitKind};

/// Fx to integer millitiles, for printing without float math.
fn milli(v: Fx) -> i64 {
    (v * Fx::lit("1000")).to_num::<i64>()
}

fn dist_from(state: &State, id: oxide_sim::UnitId, tile: TilePos) -> Fx {
    state
        .units()
        .iter()
        .find(|u| u.id == id)
        .map(|u| u.pos.dist(tile.center()))
        .unwrap_or(Fx::ZERO)
}

/// Ticks a lone unit's walk on an empty copy of the lane — the solo
/// control every drag ratio divides by.
fn solo_walk(kind: UnitKind, from: (i32, i32), goal: (i32, i32), w: usize, h: usize) -> u64 {
    let mut state = lane_arena(w, h, vec![common::unit(0, kind, from.0, from.1)])
        .build()
        .expect("lab arena builds");
    let id = state.units()[0].id;
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![id],
            goal: TilePos::new(goal.0, goal.1),
            queue: false,
        },
    )]);
    for t in 1..6_000u64 {
        state.tick(&[]);
        let u = state.units().iter().find(|u| u.id == id).unwrap();
        if u.order == Order::Idle {
            return t;
        }
    }
    6_000
}

#[test]
#[ignore = "diagnostic: the collinear head-on lane"]
fn lab_head_on_swap() {
    let (w, h) = (41, 21);
    let a_from = (5, 10);
    let b_from = (35, 10);
    let mut state = lane_arena(
        w,
        h,
        vec![
            common::unit(0, UnitKind::Sentinel, a_from.0, a_from.1),
            common::unit(0, UnitKind::Sentinel, b_from.0, b_from.1),
        ],
    )
    .build()
    .expect("builds");
    let (a, b) = (state.units()[0].id, state.units()[1].id);
    state.tick(&[
        cmd(
            0,
            Command::Move {
                units: vec![a],
                goal: TilePos::new(b_from.0, b_from.1),
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Move {
                units: vec![b],
                goal: TilePos::new(a_from.0, a_from.1),
                queue: false,
            },
        ),
    ]);
    let mut done_at = None;
    for t in 1..4_000u64 {
        state.tick(&[]);
        let idle = state
            .units()
            .iter()
            .filter(|u| u.order == Order::Idle)
            .count();
        if idle == 2 {
            done_at = Some(t);
            break;
        }
    }
    let solo = solo_walk(UnitKind::Sentinel, a_from, b_from, w, h);
    let gap = state.units()[0].pos.dist(state.units()[1].pos);
    match done_at {
        Some(t) => println!("LAB head_on_swap: both_arrived_tick={t} solo={solo}"),
        None => println!(
            "LAB head_on_swap: TIMEOUT(4000) solo={solo} final_gap_milli={} a_left_milli={} b_left_milli={}",
            milli(gap),
            milli(dist_from(&state, a, TilePos::new(b_from.0, b_from.1))),
            milli(dist_from(&state, b, TilePos::new(a_from.0, a_from.1))),
        ),
    }
}

#[test]
#[ignore = "diagnostic: fast unit overtaking a slow one on the same lane"]
fn lab_overtake() {
    let (w, h) = (41, 21);
    let mut state = lane_arena(
        w,
        h,
        vec![
            common::unit(0, UnitKind::Scuttler, 5, 10),
            common::unit(0, UnitKind::Bombard, 8, 10),
        ],
    )
    .build()
    .expect("builds");
    let (fast, slow) = (state.units()[0].id, state.units()[1].id);
    state.tick(&[
        cmd(
            0,
            Command::Move {
                units: vec![fast],
                goal: TilePos::new(37, 10),
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Move {
                units: vec![slow],
                goal: TilePos::new(33, 10),
                queue: false,
            },
        ),
    ]);
    let mut fast_done = 0u64;
    for t in 1..4_000u64 {
        state.tick(&[]);
        let u = state.units().iter().find(|u| u.id == fast).unwrap();
        if u.order == Order::Idle {
            fast_done = t;
            break;
        }
    }
    let solo = solo_walk(UnitKind::Scuttler, (5, 10), (37, 10), w, h);
    println!("LAB overtake: scuttler_arrived_tick={fast_done} solo={solo}");
}

#[test]
#[ignore = "diagnostic: two lanes crossing at 90 degrees"]
fn lab_crossing() {
    let (w, h) = (41, 21);
    let mut state = lane_arena(
        w,
        h,
        vec![
            common::unit(0, UnitKind::Sentinel, 10, 10),
            common::unit(0, UnitKind::Sentinel, 20, 3),
        ],
    )
    .build()
    .expect("builds");
    let (ew, ns) = (state.units()[0].id, state.units()[1].id);
    state.tick(&[
        cmd(
            0,
            Command::Move {
                units: vec![ew],
                goal: TilePos::new(30, 10),
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Move {
                units: vec![ns],
                goal: TilePos::new(20, 17),
                queue: false,
            },
        ),
    ]);
    let (mut ew_done, mut ns_done) = (0u64, 0u64);
    for t in 1..4_000u64 {
        state.tick(&[]);
        for (id, done) in [(ew, &mut ew_done), (ns, &mut ns_done)] {
            if *done == 0
                && state
                    .units()
                    .iter()
                    .find(|u| u.id == id)
                    .is_some_and(|u| u.order == Order::Idle)
            {
                *done = t;
            }
        }
        if ew_done > 0 && ns_done > 0 {
            break;
        }
    }
    let solo_ew = solo_walk(UnitKind::Sentinel, (10, 10), (30, 10), w, h);
    let solo_ns = solo_walk(UnitKind::Sentinel, (20, 3), (20, 17), w, h);
    println!("LAB crossing: ew_tick={ew_done} solo={solo_ew} ns_tick={ns_done} solo={solo_ns}");
}

#[test]
#[ignore = "diagnostic: eight clumped units on one group order"]
fn lab_group_march() {
    let (w, h) = (41, 21);
    let units: Vec<_> = (0..8)
        .map(|i| common::unit(0, UnitKind::Sentinel, 4 + (i % 4), 9 + (i / 4)))
        .collect();
    let mut state = lane_arena(w, h, units).build().expect("builds");
    let ids: Vec<_> = state.units().iter().map(|u| u.id).collect();
    let goal = TilePos::new(34, 10);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: ids.clone(),
            goal,
            queue: false,
        },
    )]);
    let mut first = 0u64;
    let mut last = 0u64;
    for t in 1..4_000u64 {
        state.tick(&[]);
        let idle = state
            .units()
            .iter()
            .filter(|u| u.order == Order::Idle)
            .count();
        if idle > 0 && first == 0 {
            first = t;
        }
        if idle == 8 {
            last = t;
            break;
        }
    }
    let solo = solo_walk(UnitKind::Sentinel, (4, 9), (34, 10), w, h);
    let stranded = ids
        .iter()
        .filter(|id| milli(dist_from(&state, **id, goal)) > 5_000)
        .count();
    println!(
        "LAB group_march: first_arrival={first} all_idle={last} (0=never) solo={solo} stranded_beyond_5t={stranded}"
    );
}

#[test]
#[ignore = "diagnostic: one mover ordered through a line of parked idles"]
fn lab_parked_line() {
    let (w, h) = (41, 21);
    let mut units = vec![common::unit(0, UnitKind::Sentinel, 5, 10)];
    for y in 8..=12 {
        units.push(common::unit(0, UnitKind::Sentinel, 20, y));
    }
    let mut state = lane_arena(w, h, units).build().expect("builds");
    let mover = state.units()[0].id;
    let parked: Vec<_> = state
        .units()
        .iter()
        .skip(1)
        .map(|u| (u.id, u.pos))
        .collect();
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![mover],
            goal: TilePos::new(35, 10),
            queue: false,
        },
    )]);
    let mut done = 0u64;
    for t in 1..4_000u64 {
        state.tick(&[]);
        if done == 0
            && state
                .units()
                .iter()
                .find(|u| u.id == mover)
                .is_some_and(|u| u.order == Order::Idle)
        {
            done = t;
            break;
        }
    }
    let solo = solo_walk(UnitKind::Sentinel, (5, 10), (35, 10), w, h);
    let max_shove = parked
        .iter()
        .map(|(id, start)| {
            let now = state.units().iter().find(|u| u.id == *id).unwrap().pos;
            milli(now.dist(*start))
        })
        .max()
        .unwrap_or(0);
    println!("LAB parked_line: mover_tick={done} solo={solo} max_parked_shove_milli={max_shove}");
}

#[test]
#[ignore = "diagnostic: a stationed guard vs a patrolling enemy harvester"]
fn lab_picket_kite() {
    let (w, h) = (41, 21);
    let station = TilePos::new(20, 10);
    let mut state = lane_arena(
        w,
        h,
        vec![
            common::unit(0, UnitKind::Sentinel, station.x, station.y),
            common::unit(1, UnitKind::Harvester, 32, 7),
        ],
    )
    .build()
    .expect("builds");
    let guard = state.units()[0].id;
    let bait = state.units()[1].id;
    // The guard stands its post first (a real picket forms by
    // standing, and station keeping is what arms the tether), then
    // the bait starts its dance.
    for _ in 0..60 {
        state.tick(&[]);
    }
    state.tick(&[cmd(
        1,
        Command::Patrol {
            units: vec![bait],
            waypoints: vec![TilePos::new(10, 7), TilePos::new(30, 7)],
        },
    )]);
    let mut max_drift = 0i64;
    let mut near_post = 0u64;
    let total = 6_000u64;
    for _ in 0..total {
        state.tick(&[]);
        let d = milli(dist_from(&state, guard, station));
        max_drift = max_drift.max(d);
        if d <= 3_000 {
            near_post += 1;
        }
    }
    let final_d = milli(dist_from(&state, guard, station));
    let near_permille = near_post * 1000 / total;
    println!(
        "LAB picket_kite: max_drift_milli={max_drift} final_drift_milli={final_d} near_post_permille={near_permille}"
    );
}

#[test]
#[ignore = "diagnostic: twenty scuttlers thrown at a foundry — surround utilization"]
fn lab_bulk_attack() {
    let (w, h) = (45, 27);
    let mut units = Vec::new();
    for i in 0..20 {
        units.push(common::unit(
            0,
            UnitKind::Scuttler,
            3 + (i % 2),
            8 + (i / 2),
        ));
    }
    let mut state = lane_arena(w, h, units).build().expect("builds");
    // The lab's target is the EAST foundry (2x2 at (w-3, h-3)); attack-move
    // the swarm onto it, the way a player right-clicks a base.
    let foundry = TilePos::new(w as i32 - 3, h as i32 - 3);
    let target_id = state
        .buildings()
        .iter()
        .find(|b| b.anchor == foundry)
        .expect("the east foundry stands at (w-3, h-3)")
        .id;
    let ids: Vec<_> = state.units().iter().map(|u| u.id).collect();
    state.tick(&[cmd(
        0,
        Command::AttackMove {
            units: ids.clone(),
            goal: foundry,
            queue: false,
        },
    )]);
    let mut hitters = std::collections::BTreeSet::new();
    let mut first_hit = 0u64;
    let mut contact_sum = 0u64;
    let mut contact_ticks = 0u64;
    let mut peak_contact = 0usize;
    let mut killed_at = 0u64;
    for t in 1..8_000u64 {
        let report = state.tick(&[]);
        for e in &report.events {
            if let Event::AttackHit { attacker, .. } = e {
                hitters.insert(*attacker);
                if first_hit == 0 {
                    first_hit = t;
                }
            }
        }
        if first_hit > 0 {
            // Contact census: units standing within their weapon's reach
            // of the footprint (range 0.8 = the adjacent ring).
            let reach = Fx::lit("0.8");
            let in_contact = match state.building(target_id).filter(|b| b.hp > 0) {
                Some(b) => state
                    .units()
                    .iter()
                    .filter(|u| u.hp > 0 && b.closest_point_to(u.pos).dist(u.pos) <= reach)
                    .count(),
                None => 0,
            };
            contact_sum += in_contact as u64;
            contact_ticks += 1;
            peak_contact = peak_contact.max(in_contact);
        }
        if state.building(target_id).is_none_or(|b| b.hp == 0) {
            killed_at = t;
            break;
        }
    }
    let mean_contact_milli = (contact_sum * 1000).checked_div(contact_ticks).unwrap_or(0);
    println!(
        "LAB bulk_attack: distinct_hitters={}/20 first_hit={first_hit} peak_contact={peak_contact} mean_contact_milli={mean_contact_milli} killed_at={killed_at} (0=alive at 8000)",
        hitters.len(),
    );
}

#[test]
#[ignore = "diagnostic: the same swarm against a walled base with one door"]
fn lab_bulk_attack_pocket() {
    let (w, h) = (45, 27);
    let mut units = Vec::new();
    for i in 0..20 {
        units.push(common::unit(
            0,
            UnitKind::Scuttler,
            3 + (i % 2),
            8 + (i / 2),
        ));
    }
    // A rock pocket around the east foundry with a two-tile door: the
    // player-reported shape — an army funnels in and must fan out
    // INSIDE to ring the target.
    let mut state = lane_arena_with(w, h, units, |rows| {
        for (y, row) in rows.iter_mut().enumerate().take(h - 1).skip(h - 9) {
            if !(h - 6..=h - 5).contains(&y) {
                row[w - 9] = '#';
            }
        }
        for cell in rows[h - 9].iter_mut().take(w - 1).skip(w - 9) {
            *cell = '#';
        }
    })
    .build()
    .expect("builds");
    let foundry = TilePos::new(w as i32 - 3, h as i32 - 3);
    let target_id = state
        .buildings()
        .iter()
        .find(|b| b.anchor == foundry)
        .expect("the east foundry stands at (w-3, h-3)")
        .id;
    let ids: Vec<_> = state.units().iter().map(|u| u.id).collect();
    state.tick(&[cmd(
        0,
        Command::AttackMove {
            units: ids.clone(),
            goal: foundry,
            queue: false,
        },
    )]);
    let mut hitters = std::collections::BTreeSet::new();
    let mut first_hit = 0u64;
    let mut contact_sum = 0u64;
    let mut contact_ticks = 0u64;
    let mut peak_contact = 0usize;
    let mut killed_at = 0u64;
    for t in 1..8_000u64 {
        let report = state.tick(&[]);
        for e in &report.events {
            if let Event::AttackHit { attacker, .. } = e {
                hitters.insert(*attacker);
                if first_hit == 0 {
                    first_hit = t;
                }
            }
        }
        if first_hit > 0 {
            let reach = Fx::lit("0.8");
            let in_contact = match state.building(target_id).filter(|b| b.hp > 0) {
                Some(b) => state
                    .units()
                    .iter()
                    .filter(|u| u.hp > 0 && b.closest_point_to(u.pos).dist(u.pos) <= reach)
                    .count(),
                None => 0,
            };
            contact_sum += in_contact as u64;
            contact_ticks += 1;
            peak_contact = peak_contact.max(in_contact);
        }
        if state.building(target_id).is_none_or(|b| b.hp == 0) {
            killed_at = t;
            break;
        }
    }
    let mean_contact_milli = (contact_sum * 1000).checked_div(contact_ticks).unwrap_or(0);
    println!(
        "LAB bulk_attack_pocket: distinct_hitters={}/20 first_hit={first_hit} peak_contact={peak_contact} mean_contact_milli={mean_contact_milli} killed_at={killed_at} (0=alive at 8000)",
        hitters.len(),
    );
}

#[test]
#[ignore = "diagnostic: twenty RANGED units thrown at a foundry — layer utilization"]
fn lab_bulk_attack_ranged() {
    let (w, h) = (45, 27);
    let mut units = Vec::new();
    for i in 0..20 {
        units.push(common::unit(
            0,
            UnitKind::Sentinel,
            3 + (i % 2),
            8 + (i / 2),
        ));
    }
    let mut state = lane_arena(w, h, units).build().expect("builds");
    let foundry = TilePos::new(w as i32 - 3, h as i32 - 3);
    let target_id = state
        .buildings()
        .iter()
        .find(|b| b.anchor == foundry)
        .expect("the east foundry stands at (w-3, h-3)")
        .id;
    let ids: Vec<_> = state.units().iter().map(|u| u.id).collect();
    state.tick(&[cmd(
        0,
        Command::AttackMove {
            units: ids.clone(),
            goal: foundry,
            queue: false,
        },
    )]);
    let mut hitters = std::collections::BTreeSet::new();
    let mut first_hit = 0u64;
    let mut contact_sum = 0u64;
    let mut contact_ticks = 0u64;
    let mut peak_contact = 0usize;
    let mut killed_at = 0u64;
    for t in 1..8_000u64 {
        let report = state.tick(&[]);
        for e in &report.events {
            if let Event::AttackHit { attacker, .. } = e {
                hitters.insert(*attacker);
                if first_hit == 0 {
                    first_hit = t;
                }
            }
        }
        if first_hit > 0 {
            // The sentinel fires from 2.5 tiles out: the census band is
            // its weapon reach, not the melee ring.
            let reach = Fx::lit("2.5");
            let in_contact = match state.building(target_id).filter(|b| b.hp > 0) {
                Some(b) => state
                    .units()
                    .iter()
                    .filter(|u| u.hp > 0 && b.closest_point_to(u.pos).dist(u.pos) <= reach)
                    .count(),
                None => 0,
            };
            contact_sum += in_contact as u64;
            contact_ticks += 1;
            peak_contact = peak_contact.max(in_contact);
        }
        if state.building(target_id).is_none_or(|b| b.hp == 0) {
            killed_at = t;
            break;
        }
    }
    let mean_contact_milli = (contact_sum * 1000).checked_div(contact_ticks).unwrap_or(0);
    println!(
        "LAB bulk_attack_ranged: distinct_hitters={}/20 first_hit={first_hit} peak_contact={peak_contact} mean_contact_milli={mean_contact_milli} killed_at={killed_at} (0=alive at 8000)",
        hitters.len(),
    );
}
