//! The scale bench: a mass battle for repeatable performance and
//! determinism measurements. Tests assert hashes at scale; wall-clock
//! numbers stay a local report so machine noise cannot flake a suite.

use chassis::grid::TilePos;
use chassis::grid::as_index;
use oxide_sim::scenario::{PlayerSpec, ScenarioMode, UnitSpec};
use oxide_sim::{Command, Faction, PlayerCommand, PlayerId, Scenario, UnitKind};

/// A symmetric mass battle: `per_side` mixed-role units per seat on a
/// 96x56 open field, foundries far corners, armies deployed in facing
/// blocks. Deterministic for a given (`per_side`, seed).
pub fn mass_battle(per_side: u32, seed: u64) -> Scenario {
    // A fixed mixed roster cycle keeps every combat system hot:
    // direct fire, sidearms, splash, air, anti-air.
    const CYCLE: [UnitKind; 5] = [
        UnitKind::Sentinel,
        UnitKind::Scuttler,
        UnitKind::Lancer,
        UnitKind::Flakhound,
        UnitKind::Buzzard,
    ];
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
    let row = as_index(h - 2 - 3);
    let col = as_index(w - 2 - 3);
    let mut chars: Vec<char> = map[row].chars().collect();
    chars[col] = '2';
    map[row] = chars.into_iter().collect();

    let mut units = Vec::new();
    for i in 0..per_side {
        let kind = CYCLE[(i as usize) % CYCLE.len()];
        let (dx, dy) = (
            i32::try_from(i / 24).expect("grid offsets fit in i32"),
            i32::try_from(i % 24).expect("grid offsets fit in i32"),
        );
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
            x: w - 1 - u.x,
            y: h - 1 - u.y,
        });
    }
    Scenario {
        mode: ScenarioMode::Match,
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
        buildings: Vec::new(),
        meta: None,
    }
}

/// Sends both armies through each other with crossing hunt orders. Without
/// it a bench times parked idle armies: deployment sits outside aggro range,
/// so no movement, fire, splash, or collision runs.
pub fn engage(state: &mut oxide_sim::State) {
    // The crossing goals are exact 180-degree images on the arena;
    // anything else gives the two armies different path and collision
    // geometry, and the workload is no longer symmetric.
    let goal_a = TilePos::new(80, 28);
    let goal_b = TilePos::new(96 - 1 - goal_a.x, 56 - 1 - goal_a.y);
    debug_assert_eq!((goal_b.x, goal_b.y), (15, 27));
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
            command: Command::Hunt {
                units: a,
                goal: goal_a,
                queue: false,
            },
        },
        PlayerCommand {
            player: PlayerId(1),
            command: Command::Hunt {
                units: b,
                goal: goal_b,
                queue: false,
            },
        },
    ]);
}

/// Flags every chair as a bot with the default configuration.
pub fn all_bots(scenario: &mut Scenario) {
    all_bots_with_config(scenario, oxide_sim::scenario::BotConfig::default());
}

/// Seats one exact player-facing profile in every chair of a measurement.
pub fn all_bots_with_config(scenario: &mut Scenario, config: oxide_sim::scenario::BotConfig) {
    for player in &mut scenario.players {
        player.bot = true;
        player.bot_config = Some(config);
    }
}

#[cfg(test)]
mod tests;
