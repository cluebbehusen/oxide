//! The pursuit tether (0.12): a STATIONED machine's self-acquired
//! fight carries a leash — inside [`LEASH_RADIUS`] of its anchor it
//! hunts freely, beyond it every chase tick spends a warm-blood
//! window only a joined fight refreshes — then it walks home and
//! stands a re-acquire cooldown. Station keeping takes
//! [`LEASH_STATION_TICKS`] of standing idle: a unit cycling through
//! idle mid-battle hunts unleashed, like it always did. Player
//! attacks are commitments and never tethered; any command clears
//! the tether. The counter to the measured 222-tile chase and the
//! one-harvester picket strip.

mod common;

use chassis::fx::Fx;
use chassis::grid::TilePos;
use common::*;
use oxide_sim::stats::LEASH_RADIUS;
use oxide_sim::{Command, Order, State, Target, UnitId, UnitKind};

fn milli(v: Fx) -> i64 {
    (v * Fx::lit("1000")).to_num::<i64>()
}

fn guard_dist(state: &State, id: UnitId, from: TilePos) -> i64 {
    state
        .units()
        .iter()
        .find(|u| u.id == id)
        .map(|u| milli(u.pos.dist(from.center())))
        .unwrap_or(0)
}

/// Quiet ticks: the guard earns its station standing them.
fn settle(state: &mut State, ticks: u32) {
    for _ in 0..ticks {
        state.tick(&[]);
    }
}

/// Ticks until the unit carries a leash (the fixture's acquisition
/// moment), bounded so a broken fixture fails loudly.
fn tick_until_tethered(state: &mut State, id: UnitId) {
    for _ in 0..600 {
        state.tick(&[]);
        if state
            .units()
            .iter()
            .find(|u| u.id == id)
            .is_some_and(|u| u.leash.is_some())
        {
            return;
        }
    }
    panic!("the fixture never tethered its guard");
}

#[test]
fn a_guard_breaks_off_at_the_leash_and_walks_home() {
    let station = TilePos::new(20, 10);
    let mut state = open_arena(
        41,
        21,
        vec![
            unit(0, UnitKind::Sentinel, station.x, station.y),
            unit(1, UnitKind::Harvester, 13, 8),
        ],
    )
    .build()
    .expect("builds");
    let guard = state.units()[0].id;
    let bait = state.units()[1].id;
    // The guard stands its post long enough to be stationed, then the
    // bait strolls past and keeps going — faster than every line
    // fighter, the chase could never end by catching it.
    settle(&mut state, 60);
    state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![bait],
            goal: TilePos::new(38, 8),
            queue: false,
        },
    )]);
    let mut max_drift = 0i64;
    let mut returned_at = None;
    for t in 1..3_000u64 {
        state.tick(&[]);
        let d = guard_dist(&state, guard, station);
        max_drift = max_drift.max(d);
        if returned_at.is_none() && max_drift > 2_000 && d < 500 {
            returned_at = Some(t);
        }
    }
    assert!(
        max_drift > 2_000,
        "the fixture must actually produce a chase (drift {max_drift} milli)"
    );
    // This bait clips weapon range on its way past, so blood is drawn
    // and the guard legally follows one warm window past the radius
    // (60 ticks at 0.11 ≈ 6.6 tiles). The bound is the ZONE, not the
    // line — against the un-tethered baseline's 222 tiles.
    assert!(
        max_drift <= milli(LEASH_RADIUS) + 8_000,
        "the tether binds: the guard drifted {max_drift} millitiles from its post"
    );
    let returned = returned_at.expect("the guard walks home after the break");
    let unit = state.units().iter().find(|u| u.id == guard).unwrap();
    assert_eq!(
        unit.order,
        Order::Idle,
        "home again and standing (bait long gone)"
    );
    assert!(
        unit.leash.is_none(),
        "the tether clears after the post cooldown (returned at tick {returned})"
    );
    assert!(
        guard_dist(&state, guard, station) < 500,
        "and it is actually AT the post"
    );
}

#[test]
fn a_dancing_harasser_cannot_hold_the_post_forever() {
    let station = TilePos::new(20, 10);
    let mut state = open_arena(
        41,
        21,
        vec![
            unit(0, UnitKind::Sentinel, station.x, station.y),
            unit(1, UnitKind::Harvester, 32, 7),
        ],
    )
    .build()
    .expect("builds");
    let guard = state.units()[0].id;
    let bait = state.units()[1].id;
    // Stationed first; then the bait patrols a lane clipping the
    // guard's aggro edge — the kiting exploit that held the baseline
    // picket off its post 99.6% of the time.
    settle(&mut state, 60);
    state.tick(&[cmd(
        1,
        Command::Patrol {
            units: vec![bait],
            waypoints: vec![TilePos::new(10, 7), TilePos::new(30, 7)],
        },
    )]);
    let mut max_drift = 0i64;
    for _ in 0..6_000u64 {
        state.tick(&[]);
        max_drift = max_drift.max(guard_dist(&state, guard, station));
    }
    // Inside its zone the guard may shadow the intruder indefinitely —
    // that ground is what it defends — but the ZONE bounds it: the
    // baseline guard followed this same dance wherever it led.
    assert!(
        max_drift <= milli(LEASH_RADIUS) + 2_000,
        "even a dancer cannot drag the guard far past the tether ({max_drift} milli)"
    );
    // And the moment the bait actually leaves, the post is recovered.
    state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![bait],
            goal: TilePos::new(38, 18),
            queue: false,
        },
    )]);
    let mut recovered = false;
    for _ in 0..1_200u64 {
        state.tick(&[]);
        let u = state.units().iter().find(|u| u.id == guard).unwrap();
        // Two honest endings: the bait escaped and the guard walked
        // home (break-off), or a turnaround carried the bait through
        // weapon range once too often and the guard killed it —
        // victory stands its ground, still inside the zone.
        let bait_dead = state
            .units()
            .iter()
            .find(|u| u.id == bait)
            .is_none_or(|u| u.hp == 0);
        if u.order == Order::Idle
            && u.leash.is_none()
            && (guard_dist(&state, guard, station) < 500 || bait_dead)
        {
            recovered = true;
            break;
        }
    }
    assert!(
        recovered,
        "the bait left; the guard settles — home, or standing over the kill"
    );
}

#[test]
fn the_leash_outreaches_the_bombard() {
    // The one constraint that keeps siege weapons answerable: a guard
    // whose tether were shorter than the Bombard's reach would turn
    // back before ever touching the gun shelling it.
    let bombard_range = UnitKind::Bombard.stats().weapons[0].range;
    assert!(
        LEASH_RADIUS >= bombard_range,
        "LEASH_RADIUS must cover the longest weapon reach"
    );
}

#[test]
fn a_returning_guard_answers_fire() {
    let station = TilePos::new(5, 10);
    let mut state = open_arena(
        41,
        21,
        vec![
            unit(0, UnitKind::Sentinel, station.x, station.y),
            unit(1, UnitKind::Harvester, 14, 10),
            unit(1, UnitKind::Scuttler, 13, 12),
        ],
    )
    .build()
    .expect("builds");
    let guard = state.units()[0].id;
    let bait = state.units()[1].id;
    let raider = state.units()[2].id;
    // Stationed first. The bait walks in, hooks the guard, and drags
    // it east past the tether; the raider (a contact-range shredder)
    // picks the guard up en route and keeps hitting it through the
    // break. The homecoming must answer — a guard that eats a pursuit
    // home without firing back is SC2's documented vulnerability
    // window, reproduced.
    settle(&mut state, 60);
    state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![bait],
            goal: TilePos::new(9, 10),
            queue: false,
        },
    )]);
    state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![bait],
            goal: TilePos::new(38, 10),
            queue: true,
        },
    )]);
    let mut answered = false;
    for _ in 0..3_000u64 {
        state.tick(&[]);
        let Some(u) = state.units().iter().find(|u| u.id == guard) else {
            break;
        };
        if matches!(u.order, Order::Attack { target: Target::Unit(t), .. } if t == raider) {
            answered = true;
        }
    }
    assert!(
        answered,
        "the guard turned on the raider that hit it instead of eating the walk home"
    );
    // Victory stands its ground: the raider dies (60 hp sentinel vs a
    // fragile shredder) and the survivor holds where the fight ended —
    // inside the zone, still stationed, so the next intruder tethers
    // it right there. (Walking home after every kill was measured to
    // lose base defenses; only BREAK-offs walk home.)
    let raider_dead = state
        .units()
        .iter()
        .find(|u| u.id == raider)
        .is_none_or(|u| u.hp == 0);
    let survivor = state
        .units()
        .iter()
        .find(|u| u.id == guard)
        .filter(|u| u.hp > 0);
    if raider_dead && let Some(u) = survivor {
        assert!(
            guard_dist(&state, guard, station) <= milli(LEASH_RADIUS) + 8_000,
            "the survivor stands inside the bounded zone"
        );
        assert!(
            u.leash.is_none() && u.settled > 0,
            "the victor is unleashed but still counts as stationed"
        );
    }
}

#[test]
fn a_player_attack_is_never_leashed() {
    let mut state = open_arena(
        41,
        21,
        vec![
            unit(0, UnitKind::Sentinel, 5, 10),
            unit(1, UnitKind::Harvester, 8, 10),
        ],
    )
    .build()
    .expect("builds");
    let hunter = state.units()[0].id;
    let prey = state.units()[1].id;
    state.tick(&[
        cmd(
            0,
            Command::Attack {
                units: vec![hunter],
                target: Target::Unit(prey),
                queue: false,
            },
        ),
        cmd(
            1,
            Command::Move {
                units: vec![prey],
                goal: TilePos::new(37, 10),
                queue: false,
            },
        ),
    ]);
    for _ in 0..2_500u64 {
        state.tick(&[]);
    }
    let u = state.units().iter().find(|u| u.id == hunter).unwrap();
    assert!(
        u.leash.is_none(),
        "an explicit attack is a commitment — no tether"
    );
    assert!(
        milli(u.pos.dist(TilePos::new(5, 10).center())) > milli(LEASH_RADIUS),
        "and the chase crossed where any leash would have broken it"
    );
}

#[test]
fn a_battle_cycling_unit_hunts_unleashed() {
    // Mid-battle idles re-acquire without a tether: leashing them once
    // turned scripted army fights into seat-parity coin flips. A unit
    // that JUST went idle (settled below the station threshold) picks
    // its next fight exactly like the pre-tether sim.
    let mut state = open_arena(
        41,
        21,
        vec![
            unit(0, UnitKind::Sentinel, 20, 10),
            unit(1, UnitKind::Harvester, 24, 10),
        ],
    )
    .build()
    .expect("builds");
    let fighter = state.units()[0].id;
    // No settling: the enemy is in aggro from the first brain tick,
    // long before the station threshold.
    state.tick(&[]);
    let u = state.units().iter().find(|u| u.id == fighter).unwrap();
    assert!(
        matches!(u.order, Order::Attack { .. }),
        "the fight starts immediately"
    );
    assert!(
        u.leash.is_none(),
        "an unsettled machine hunts unleashed — no tether on battle cycling"
    );
}

#[test]
fn reissuing_the_selfsame_attack_clears_the_tether() {
    let mut state = open_arena(
        41,
        21,
        vec![
            unit(0, UnitKind::Sentinel, 20, 10),
            unit(1, UnitKind::Harvester, 28, 10),
        ],
    )
    .build()
    .expect("builds");
    let guard = state.units()[0].id;
    let prey = state.units()[1].id;
    // Stationed, then the prey wanders in and the guard picks the
    // fight itself.
    settle(&mut state, 60);
    state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![prey],
            goal: TilePos::new(22, 10),
            queue: false,
        },
    )]);
    tick_until_tethered(&mut state, guard);
    let u = state.units().iter().find(|u| u.id == guard).unwrap();
    assert!(
        matches!(u.order, Order::Attack { target: Target::Unit(t), resume: None } if t == prey),
        "the guard self-acquired its visitor"
    );
    // The player blesses the same fight: the order compares equal (the
    // no-op path — path and progress survive), but the tether must
    // clear — this is now a commitment.
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![guard],
            target: Target::Unit(prey),
            queue: false,
        },
    )]);
    let u = state.units().iter().find(|u| u.id == guard).unwrap();
    assert!(
        matches!(u.order, Order::Attack { target: Target::Unit(t), .. } if t == prey),
        "the order itself is untouched"
    );
    assert!(
        u.leash.is_none(),
        "the no-op reissue still cleared the tether"
    );
}

#[test]
fn stop_drops_the_tether() {
    let mut state = open_arena(
        41,
        21,
        vec![
            unit(0, UnitKind::Sentinel, 20, 10),
            unit(1, UnitKind::Harvester, 28, 10),
        ],
    )
    .build()
    .expect("builds");
    let guard = state.units()[0].id;
    let prey = state.units()[1].id;
    // Stationed; the prey walks in to hook the guard, then runs for
    // the far wall so the Stop lands with nothing left in aggro —
    // otherwise idle() legitimately re-engages the same tick.
    settle(&mut state, 60);
    state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![prey],
            goal: TilePos::new(22, 10),
            queue: false,
        },
    )]);
    tick_until_tethered(&mut state, guard);
    state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![prey],
            goal: TilePos::new(38, 10),
            queue: false,
        },
    )]);
    for _ in 0..200 {
        state.tick(&[]);
    }
    state.tick(&[cmd(0, Command::Stop { units: vec![guard] })]);
    let u = state.units().iter().find(|u| u.id == guard).unwrap();
    assert!(u.leash.is_none(), "Stop is a command like any other");
    assert_eq!(u.order, Order::Idle);
}
