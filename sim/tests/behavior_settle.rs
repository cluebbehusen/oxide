//! Crowd settling: measured displacement in steady state — the 0.12
//! jitter phase's instrument, and its regression once the dials land.

mod common;

use chassis::grid::TilePos;
use oxide_sim::{Command, UnitKind};

use common::*;

/// Sum of per-unit displacement magnitudes over one tick, in
/// thousandths of a tile (integer — the probe must not do float math
/// on sim values).
fn tick_displacement(state: &mut oxide_sim::State) -> u64 {
    let before: Vec<_> = state.units().iter().map(|u| (u.id, u.pos)).collect();
    state.tick(&[]);
    let mut total = 0u64;
    for (id, old) in before {
        if let Some(u) = state.units().iter().find(|u| u.id == id) {
            let d = u.pos - old;
            let sq = d.length_sq();
            // Raw Q32.32 bits: zero means EXACTLY stationary, and the
            // probe only compares relative magnitudes.
            total += sq.to_bits().max(0) as u64;
        }
    }
    total
}

/// Eight harvesters magnetized to one node: the doorstep crowd from
/// the relaxer's own docstring. The steady state may jostle while
/// extraction rotates the crew, but a SETTLED crowd — the parked blob
/// below — must go fully stationary.
#[test]
#[ignore = "diagnostic: prints steady-state displacement for the jitter work"]
fn crowded_harvest_steady_state() {
    let units: Vec<_> = (0..8)
        .map(|i| unit(0, UnitKind::Harvester, 2 + (i % 4), 5 + (i / 4)))
        .collect();
    let mut state = arena(units).build().unwrap();
    let ids: Vec<_> = state.units().iter().map(|u| u.id).collect();
    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: ids,
            node: TilePos::new(11, 4),
            queue: false,
        },
    )]);
    for _ in 0..1_500 {
        state.tick(&[]);
    }
    let mut samples = Vec::new();
    for _ in 0..300 {
        samples.push(tick_displacement(&mut state));
    }
    let nonzero = samples.iter().filter(|&&s| s > 0).count();
    let mean: u64 = samples.iter().sum::<u64>() / samples.len() as u64;
    println!("crowded harvest: {nonzero}/300 moving ticks, mean {mean} (millitile^2-ish)");
}

/// A parked blob: ten harvesters group-moved to a cluster, then left
/// alone. Once arrival propagation settles them, every later tick must
/// leave every position EXACTLY where it was — settled machines do not
/// vibrate. This is the regression the deadband/settle work gates on.
#[test]
fn a_parked_crowd_goes_fully_stationary() {
    let units: Vec<_> = (0..10)
        .map(|i| unit(0, UnitKind::Harvester, 2 + (i % 5), 5 + (i / 5)))
        .collect();
    let mut state = arena(units).build().unwrap();
    let ids: Vec<_> = state.units().iter().map(|u| u.id).collect();
    state.tick(&[cmd(
        0,
        Command::Move {
            units: ids,
            goal: TilePos::new(9, 2),
            queue: false,
        },
    )]);
    // Ample time to arrive and settle (the arena is 16x9).
    for _ in 0..1_200 {
        state.tick(&[]);
    }
    let mut still = 0;
    let mut moving_ticks = Vec::new();
    for t in 0..200 {
        let d = tick_displacement(&mut state);
        if d == 0 {
            still += 1;
        } else {
            moving_ticks.push((t, d));
        }
    }
    assert!(
        state
            .units()
            .iter()
            .all(|u| matches!(u.order, oxide_sim::Order::Idle)),
        "precondition: everyone arrived and idles"
    );
    assert_eq!(
        still,
        200,
        "a parked crowd still vibrates: {} moving ticks, first {:?}",
        moving_ticks.len(),
        moving_ticks.first()
    );
}
