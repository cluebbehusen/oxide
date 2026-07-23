//! Par-cost arena duels: two hand-picked armies on flat ground, no
//! economy, attack-moved into each other — the controlled experiment
//! that separates "the learner never found the counter" from "no
//! counter exists". Surviving army value is the verdict.

use anyhow::{Context, Result, bail};
use chassis::grid::TilePos;
use oxide_sim::scenario::{PlayerSpec, UnitSpec};
use oxide_sim::{Command, Faction, PlayerCommand, PlayerId, Scenario, UnitKind};

/// One side's shopping list.
pub type Army = Vec<(UnitKind, u32)>;

/// Parses "sentinel:10,lancer:4" into an army.
pub fn parse_army(spec: &str) -> Result<Army> {
    let mut army = Vec::new();
    for part in spec.split(',') {
        let (name, count) = part
            .split_once(':')
            .with_context(|| format!("'{part}' wants kind:count"))?;
        let kind = ALL_KINDS
            .iter()
            .copied()
            .find(|k| k.name().eq_ignore_ascii_case(name.trim()))
            .with_context(|| format!("unknown unit kind '{name}'"))?;
        army.push((kind, count.trim().parse()?));
    }
    Ok(army)
}

const ALL_KINDS: [UnitKind; 11] = [
    UnitKind::Harvester,
    UnitKind::Sentinel,
    UnitKind::Scuttler,
    UnitKind::Lancer,
    UnitKind::Bombard,
    UnitKind::Flakhound,
    UnitKind::Buzzard,
    UnitKind::Talon,
    UnitKind::Stinger,
    UnitKind::Darter,
    UnitKind::Wisp,
];

/// Total scrap an army costs.
pub fn army_cost(army: &Army) -> u32 {
    army.iter().map(|(kind, n)| kind.stats().cost * n).sum()
}

/// The duel's outcome: surviving army value per side after the dust
/// settles (zero on both sides is mutual annihilation).
#[derive(Debug, Clone, serde::Serialize)]
pub struct DuelOutcome {
    /// Side A's surviving value.
    pub a_value: u32,
    /// Side B's surviving value.
    pub b_value: u32,
    /// Ticks until quiet (no side lost value for a stretch).
    pub ticks: u64,
}

/// Runs one duel on an open arena: armies deploy in mirrored lines and
/// attack-move through each other's positions.
pub fn duel(a: &Army, b: &Army, seed: u64, max_ticks: u64) -> Result<DuelOutcome> {
    let width = 40;
    let height = 24;
    let mut map = Vec::new();
    for y in 0..height {
        let row: String = (0..width)
            .map(|x| {
                if y == 0 || y == height - 1 || x == 0 || x == width - 1 {
                    '#'
                } else if (x, y) == (2, 2) {
                    '1'
                } else if (x, y) == (width - 4, height - 3) {
                    '2'
                } else {
                    '.'
                }
            })
            .collect();
        map.push(row);
    }
    let mut units = Vec::new();
    let mut place = |army: &Army, player: u8, x0: i32| {
        let mut i = 0i32;
        for (kind, n) in army {
            for _ in 0..*n {
                units.push(UnitSpec {
                    player,
                    kind: *kind,
                    x: x0 + (i / 16),
                    y: 4 + (i % 16),
                });
                i += 1;
            }
        }
    };
    place(a, 0, 8);
    place(b, 1, 30);
    let scenario = Scenario {
        name: "arena-duel".into(),
        seed,
        map,
        players: vec![seat("A", Faction::Ferrous), seat("B", Faction::Cupric)],
        units,
        meta: None,
    };
    let mut state = scenario.build().context("arena builds")?;
    let (a_ids, b_ids): (Vec<_>, Vec<_>) = {
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
    if a_ids.is_empty() || b_ids.is_empty() {
        bail!("both sides need units");
    }
    state.tick(&[
        PlayerCommand {
            player: PlayerId(0),
            command: Command::AttackMove {
                units: a_ids,
                goal: TilePos::new(33, 12),
                queue: false,
            },
        },
        PlayerCommand {
            player: PlayerId(1),
            command: Command::AttackMove {
                units: b_ids,
                goal: TilePos::new(6, 12),
                queue: false,
            },
        },
    ]);
    let value = |state: &oxide_sim::State, player: u8| -> u32 {
        state
            .units()
            .iter()
            .filter(|u| u.player == PlayerId(player) && u.kind != UnitKind::Harvester)
            .map(|u| u.kind.stats().cost)
            .sum()
    };
    let mut last = (value(&state, 0), value(&state, 1));
    let mut quiet = 0u64;
    let mut ran = 1;
    for _ in 1..max_ticks {
        state.tick(&[]);
        ran += 1;
        let now = (value(&state, 0), value(&state, 1));
        if now == last {
            quiet += 1
        } else {
            quiet = 0
        }
        last = now;
        // One side wiped, or nothing has changed for 15 seconds of
        // sim time: the fight is over.
        if now.0 == 0 || now.1 == 0 || quiet > 300 {
            break;
        }
    }
    Ok(DuelOutcome {
        a_value: last.0,
        b_value: last.1,
        ticks: ran,
    })
}

fn seat(name: &str, faction: Faction) -> PlayerSpec {
    PlayerSpec {
        name: name.into(),
        faction,
        team: None,
        scrap: 0,
        bot: false,
        bot_config: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirrored_armies_annihilate_or_split_close() {
        let army = parse_army("sentinel:6").unwrap();
        let out = duel(&army, &army, 42, 6_000).unwrap();
        let spread = out.a_value.abs_diff(out.b_value);
        assert!(
            spread <= army_cost(&army) / 3,
            "a mirror should end near even, got {out:?}"
        );
    }

    #[test]
    fn the_parser_speaks_kind_names_and_counts() {
        let army = parse_army("Sentinel:3, lancer:2").unwrap();
        assert_eq!(army.len(), 2);
        assert_eq!(
            army_cost(&army),
            3 * UnitKind::Sentinel.stats().cost + 2 * UnitKind::Lancer.stats().cost
        );
        assert!(parse_army("gremlin:4").is_err());
    }
}
