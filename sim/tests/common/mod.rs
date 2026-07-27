//! Shared scaffolding for the behavior suites (split by family
//! in the 0.10 file diet; each binary uses its own subset).
#![allow(dead_code)]

use oxide_sim::scenario::{PlayerSpec, UnitSpec};
use oxide_sim::{Command, Event, Faction, PlayerCommand, PlayerId, Scenario, State, UnitKind};

/// A small arena: two Foundries in opposite corners, open ground between.
pub fn arena(units: Vec<UnitSpec>) -> Scenario {
    Scenario {
        name: "test-arena".into(),
        seed: 42,
        map: vec![
            "################".into(),
            "#1.............#".into(),
            "#..............#".into(),
            "#.....##.......#".into(),
            "#.....##...s...#".into(),
            "#..........s...#".into(),
            "#............2.#".into(),
            "#..............#".into(),
            "################".into(),
        ],
        players: vec![
            PlayerSpec {
                name: "Ferrous".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap: 200,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "Cupric".into(),
                faction: Faction::Cupric,
                team: None,
                scrap: 200,
                bot: false,
                bot_config: None,
            },
        ],
        units,
        buildings: Vec::new(),
        meta: None,
    }
}

pub fn unit(player: u8, kind: UnitKind, x: i32, y: i32) -> UnitSpec {
    UnitSpec { player, kind, x, y }
}

/// A wide open arena — Foundries tucked into opposite corners, out of
/// every lane — for movement and pursuit scenarios that need real
/// distances. `carve` edits the char grid (walls, doors) after the
/// open fill, before the anchors land.
pub fn open_arena_with(
    width: usize,
    height: usize,
    units: Vec<UnitSpec>,
    carve: impl Fn(&mut Vec<Vec<char>>),
) -> Scenario {
    let mut rows = vec![vec!['#'; width]; height];
    for row in rows.iter_mut().take(height - 1).skip(1) {
        for cell in row.iter_mut().take(width - 1).skip(1) {
            *cell = '.';
        }
    }
    carve(&mut rows);
    rows[1][1] = '1';
    rows[height - 3][width - 3] = '2';
    Scenario {
        name: "open-arena".into(),
        seed: 42,
        map: rows.into_iter().map(|r| r.into_iter().collect()).collect(),
        players: vec![
            PlayerSpec {
                name: "West".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "East".into(),
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

/// [`open_arena_with`] without terrain edits.
pub fn open_arena(width: usize, height: usize, units: Vec<UnitSpec>) -> Scenario {
    open_arena_with(width, height, units, |_| {})
}

pub fn cmd(player: u8, command: Command) -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(player),
        command,
    }
}

/// Runs until `stop` returns true or `max_ticks` elapse, collecting every
/// event. Panics if the condition never holds — behavior tests should state
/// exactly how long something may take.
pub fn run_until(
    state: &mut State,
    max_ticks: u64,
    mut stop: impl FnMut(&State, &[Event]) -> bool,
) -> Vec<Event> {
    let mut all = Vec::new();
    for _ in 0..max_ticks {
        let report = state.tick(&[]);
        let done = stop(state, &report.events);
        all.extend(report.events);
        if done {
            return all;
        }
    }
    panic!("condition not reached within {max_ticks} ticks");
}
