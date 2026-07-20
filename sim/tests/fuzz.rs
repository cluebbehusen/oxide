//! Command fuzzing: the sim's command surface faces the debug socket, so
//! it must shrug off hostile input — garbage ids, out-of-range players,
//! coordinates at the integer extremes — without panicking, and the same
//! seeded garbage must produce the same bits on every run.

use chassis::grid::TilePos;
use chassis::rng::Pcg32;
use oxide_sim::{BuildingId, Command, PlayerCommand, PlayerId, Scenario, Target, UnitId, UnitKind};

const TICKS: u64 = 5_000;

/// A coordinate that is usually plausible and occasionally adversarial.
fn coord(rng: &mut Pcg32, edge: i32) -> i32 {
    match rng.next_below(10) {
        0 => i32::MAX,
        1 => i32::MIN,
        2 => -(rng.next_below(100) as i32),
        3 => edge + rng.next_below(100) as i32,
        _ => rng.next_below(edge as u32) as i32,
    }
}

fn tile(rng: &mut Pcg32) -> TilePos {
    // Skirmish is 40x24; most rolls land inside it.
    TilePos::new(coord(rng, 40), coord(rng, 24))
}

/// Unit ids sweep live ones, soon-to-exist ones, and pure garbage.
fn units(rng: &mut Pcg32) -> Vec<UnitId> {
    (0..rng.next_below(5))
        .map(|_| {
            UnitId(match rng.next_below(8) {
                0 => rng.next_u32(),
                _ => rng.next_below(64),
            })
        })
        .collect()
}

fn command(rng: &mut Pcg32) -> PlayerCommand {
    // Players 0-3 on a two-player map: half the issuers don't exist.
    let player = PlayerId(rng.next_below(4) as u8);
    let command = match rng.next_below(8) {
        0 => Command::Move {
            units: units(rng),
            goal: tile(rng),
            queue: rng.next_below(2) == 0,
        },
        1 => Command::Attack {
            units: units(rng),
            target: if rng.next_below(2) == 0 {
                Target::Unit(UnitId(rng.next_below(80)))
            } else {
                Target::Building(BuildingId(rng.next_below(8)))
            },
            queue: rng.next_below(2) == 0,
        },
        2 => Command::AttackMove {
            units: units(rng),
            goal: tile(rng),
            queue: rng.next_below(2) == 0,
        },
        3 => Command::Harvest {
            units: units(rng),
            node: tile(rng),
            queue: rng.next_below(2) == 0,
        },
        4 => Command::Stop { units: units(rng) },
        7 => Command::Patrol {
            units: units(rng),
            waypoints: (0..rng.next_below(5)).map(|_| tile(rng)).collect(),
        },
        5 => Command::Train {
            building: BuildingId(rng.next_below(8)),
            kind: if rng.next_below(2) == 0 {
                UnitKind::Harvester
            } else {
                UnitKind::Sentinel
            },
        },
        _ => Command::SetRally {
            building: BuildingId(rng.next_below(8)),
            rally: if rng.next_below(3) == 0 {
                None
            } else {
                Some(tile(rng))
            },
        },
    };
    PlayerCommand { player, command }
}

fn fuzz_run(seed: u64) -> u64 {
    let mut state = Scenario::skirmish().build().unwrap();
    let mut rng = Pcg32::new(seed, 0xF022);
    for _ in 0..TICKS {
        let commands: Vec<PlayerCommand> =
            (0..rng.next_below(4)).map(|_| command(&mut rng)).collect();
        state.tick(&commands);
    }
    state.hash()
}

#[test]
fn seeded_garbage_never_panics_and_reproduces() {
    assert_eq!(
        fuzz_run(0xDEC0DE),
        fuzz_run(0xDEC0DE),
        "same seeded command stream, different bits"
    );
}

#[test]
fn a_second_seed_widens_the_net() {
    assert_eq!(fuzz_run(20260719), fuzz_run(20260719));
}
