//! The scale bench: a mass battle big enough to make performance and
//! determinism claims repeatable. CI asserts correctness at scale
//! (hashes, briefly); wall-clock numbers stay a local report so
//! machine noise can never flake a suite.

use oxide_sim::scenario::{PlayerSpec, UnitSpec};
use oxide_sim::{Faction, Scenario, UnitKind};

/// A symmetric mass battle: `per_side` mixed-role units per seat on a
/// 96x56 open field, foundries far corners, armies deployed in facing
/// blocks. Deterministic for a given (per_side, seed).
pub fn mass_battle(per_side: u32, seed: u64) -> Scenario {
    let (w, h) = (96, 56);
    let mut map: Vec<String> = (0..h)
        .map(|y| {
            (0..w)
                .map(|x| {
                    if y == 0 || y == h - 1 || x == 0 || x == w - 1 {
                        '#'
                    } else if (x, y) == (3, 3) {
                        '1'
                    } else {
                        '.'
                    }
                })
                .collect()
        })
        .collect();
    // Seat 2's anchor at the footprint mirror.
    let row = (h - 2 - 3) as usize;
    let col = (w - 2 - 3) as usize;
    let mut chars: Vec<char> = map[row].chars().collect();
    chars[col] = '2';
    map[row] = chars.into_iter().collect();

    // A fixed mixed roster cycle keeps every combat system hot:
    // direct fire, sidearms, splash, air, anti-air.
    const CYCLE: [UnitKind; 5] = [
        UnitKind::Sentinel,
        UnitKind::Scuttler,
        UnitKind::Lancer,
        UnitKind::Flakhound,
        UnitKind::Buzzard,
    ];
    let mut units = Vec::new();
    for i in 0..per_side {
        let kind = CYCLE[(i as usize) % CYCLE.len()];
        let (dx, dy) = ((i / 24) as i32, (i % 24) as i32);
        units.push(UnitSpec {
            player: 0,
            kind,
            x: 12 + dx,
            y: 16 + dy,
        });
    }
    for u in units.clone() {
        units.push(UnitSpec {
            player: 1,
            kind: u.kind,
            x: (w - 1) as i32 - u.x,
            y: (h - 1) as i32 - u.y,
        });
    }
    Scenario {
        name: format!("mass-battle-{per_side}"),
        seed,
        map,
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
        units,
        meta: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chassis::grid::TilePos;
    use oxide_sim::{Command, PlayerCommand, PlayerId};

    /// Two identical runs at scale, hash-compared every 50 ticks — the
    /// CI face of the bench. Short on purpose: the timed thousands-of-
    /// ticks run is the CLI's job on a dev machine.
    #[test]
    fn five_hundred_units_stay_bit_identical_across_runs() {
        let run = || {
            let scenario = mass_battle(250, 9);
            let mut state = scenario.build().expect("scale scenario builds");
            let (a, b): (Vec<_>, Vec<_>) = {
                let units = state.units();
                (
                    units
                        .iter()
                        .filter(|u| u.player == PlayerId(0))
                        .map(|u| u.id)
                        .collect(),
                    units
                        .iter()
                        .filter(|u| u.player == PlayerId(1))
                        .map(|u| u.id)
                        .collect(),
                )
            };
            state.tick(&[
                PlayerCommand {
                    player: PlayerId(0),
                    command: Command::AttackMove {
                        units: a,
                        goal: TilePos::new(80, 28),
                        queue: false,
                    },
                },
                PlayerCommand {
                    player: PlayerId(1),
                    command: Command::AttackMove {
                        units: b,
                        goal: TilePos::new(15, 28),
                        queue: false,
                    },
                },
            ]);
            let mut hashes = Vec::new();
            for tick in 1..=200u32 {
                state.tick(&[]);
                if tick % 50 == 0 {
                    hashes.push(state.hash());
                }
            }
            hashes
        };
        assert_eq!(run(), run(), "scale must not cost determinism");
    }
}
