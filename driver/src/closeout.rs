//! The close-out probe: can the shipped actor finish a won game?
//!
//! Every stalled map in the 0.15 pool health pass shared one signature
//! — overwhelming force that never kills the remnant. This instrument
//! builds that endgame directly: a dominant seat with an army against
//! a bare remnant Foundry across the map, in two deterministic
//! variants. In `intel`, a parked scout flyer hands the dominant seat
//! sight of the remnant from tick zero, so the Push action's
//! known-enemy-site requirement is satisfied by honest fog knowledge.
//! In `dark`, the remnant has never been seen — closing requires the
//! actor to go looking first. The split separates "cannot finish" from
//! "cannot find": a fast intel kill with a dark stall is a scouting
//! failure; a stall in both is a commitment failure. Diagnostic only —
//! observed, never rewarded.

use anyhow::{Context, Result};
use oxide_sim::bot::{Level, NeuralBot, QuantNet};
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::stats::BuildingKind;
use oxide_sim::{Faction, PlayerId, Scenario, UnitKind};

fn fixture(seed: u64, intel: bool, far: bool, defended: bool) -> Scenario {
    let (w, h) = if far { (150, 100) } else { (60, 40) };
    let mut row = String::from("#");
    row.push_str(&".".repeat(w - 2));
    row.push('#');
    let mut map: Vec<String> = vec!["#".repeat(w)];
    for _ in 0..(h - 2) {
        map.push(row.clone());
    }
    map.push("#".repeat(w));
    let (rx, ry) = (w - 5, h - 6);
    let map: Vec<String> = {
        let mut grid: Vec<Vec<char>> = map.iter().map(|r| r.chars().collect()).collect();
        grid[4][3] = '1';
        grid[ry][rx] = '2';
        // A little home economy for the dominant seat; the remnant
        // gets bare ground and its lone harvester.
        for (x, y) in [(8, 8), (9, 8), (8, 9)] {
            grid[y][x] = 's';
        }
        grid.into_iter().map(|r| r.into_iter().collect()).collect()
    };
    let mut units = vec![
        UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x: 6,
            y: 6,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x: 7,
            y: 6,
        },
    ];
    for i in 0..8 {
        units.push(UnitSpec {
            player: 0,
            kind: UnitKind::Lancer,
            x: 12 + (i % 4),
            y: 10 + (i / 4),
        });
    }
    for i in 0..4 {
        units.push(UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 12 + i,
            y: 12,
        });
    }
    if intel {
        // The parked scout: honest sight of the remnant from tick
        // zero, and ghost memory thereafter.
        units.push(UnitSpec {
            player: 0,
            kind: UnitKind::Kestrel,
            x: (rx - 3) as i32,
            y: (ry - 2) as i32,
        });
    }
    units.push(UnitSpec {
        player: 1,
        kind: UnitKind::Harvester,
        x: (rx - 1) as i32,
        y: (ry + 3) as i32,
    });
    let mut buildings = vec![BuildingSpec {
        player: 0,
        kind: BuildingKind::Fabricator,
        x: 10,
        y: 4,
    }];
    if defended {
        buildings.push(BuildingSpec {
            player: 1,
            kind: BuildingKind::Turret,
            x: (rx - 4) as i32,
            y: (ry - 1) as i32,
        });
        buildings.push(BuildingSpec {
            player: 1,
            kind: BuildingKind::Turret,
            x: (rx + 1) as i32,
            y: (ry + 3) as i32,
        });
        units.push(UnitSpec {
            player: 1,
            kind: UnitKind::Sentinel,
            x: (rx - 2) as i32,
            y: (ry + 4) as i32,
        });
    }
    Scenario {
        name: format!("closeout-{}", if intel { "intel" } else { "dark" }),
        seed,
        map,
        players: vec![
            PlayerSpec {
                name: "Dominant".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap: 400,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "Remnant".into(),
                faction: Faction::Cupric,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
        ],
        units,
        buildings,
        meta: None,
    }
}

/// Runs both variants across the seed suite and prints JSON rows.
pub fn closeout_probe(
    weights: &std::path::Path,
    seeds: u64,
    max_ticks: u64,
    blunder: u32,
    cadence: u64,
) -> Result<()> {
    let json = std::fs::read_to_string(weights)
        .with_context(|| format!("reading {}", weights.display()))?;
    let net = QuantNet::from_json(&json).map_err(|e| anyhow::anyhow!(e))?;
    eprintln!(
        "closeout probe: {} · digest {:016x} · {} seeds x 2 variants · {}t horizon",
        weights.display(),
        net.digest(),
        seeds,
        max_ticks
    );
    for (intel, far, defended) in [
        (true, false, false),
        (false, false, false),
        (true, true, false),
        (false, true, false),
        (true, true, true),
        (false, true, true),
    ] {
        let variant = format!(
            "{}-{}{}",
            if intel { "intel" } else { "dark" },
            if far { "far" } else { "near" },
            if defended { "-defended" } else { "" }
        );
        let mut decided = 0u64;
        let mut ticks_when_decided = Vec::new();
        for seed in 3000..3000 + seeds {
            let scenario = fixture(seed, intel, far, defended);
            let mut state = scenario.build().map_err(|e| anyhow::anyhow!("{e}"))?;
            let mut bot = NeuralBot::with_profile_hesitation(
                PlayerId(0),
                cadence,
                net.clone(),
                Level::Expert.skill(),
                550,
                Faction::Ferrous,
                Some(blunder),
                seed,
            );
            let mut end = None;
            for _ in 0..max_ticks {
                let commands = bot.act(&state);
                state.tick(&commands);
                if state.result().is_some() {
                    end = Some(state.current_tick());
                    break;
                }
            }
            if let Some(t) = end {
                decided += 1;
                ticks_when_decided.push(t);
            }
        }
        ticks_when_decided.sort_unstable();
        let median = ticks_when_decided
            .get(ticks_when_decided.len() / 2)
            .copied();
        println!(
            "{}",
            serde_json::json!({
                "variant": variant,
                "decided": decided,
                "games": seeds,
                "median_ticks": median,
            })
        );
    }
    Ok(())
}
