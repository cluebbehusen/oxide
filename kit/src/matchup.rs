//! Par-cost arena duels: two hand-picked armies on flat ground, no
//! economy, attack-moved into each other — the controlled experiment
//! that separates "the learner never found the counter" from "no
//! counter exists". Surviving army value is the verdict.
//!
//! Defense mode stands pre-built structures in front of side B: the
//! swarm-vs-fortification experiment. Deliberately asymmetric — the
//! mirror-fairness rule applies to duels, not sieges, and the garrison
//! value counts toward B's verdict like any purchase.

use anyhow::{Context, Result, bail};
use chassis::grid::TilePos;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::{BuildingKind, Command, Faction, PlayerCommand, PlayerId, Scenario, UnitKind};

/// One side's shopping list.
pub type Army = Vec<(UnitKind, u32)>;

/// A defending side's structure list.
pub type Garrison = Vec<(BuildingKind, u32)>;

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

/// Parses "turret:3,bastion:1" into a garrison. Foundries are refused:
/// the arena's own anchors are the victory tokens, and the verdict
/// deliberately never counts them.
pub fn parse_garrison(spec: &str) -> Result<Garrison> {
    const KINDS: [BuildingKind; 6] = [
        BuildingKind::Turret,
        BuildingKind::Fabricator,
        BuildingKind::FlakTurret,
        BuildingKind::Bastion,
        BuildingKind::Array,
        BuildingKind::Reclaimer,
    ];
    let squash = |s: &str| {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect::<String>()
    };
    let mut garrison = Vec::new();
    for part in spec.split(',') {
        let (name, count) = part
            .split_once(':')
            .with_context(|| format!("'{part}' wants kind:count"))?;
        let kind = KINDS
            .iter()
            .copied()
            .find(|k| squash(k.name()) == squash(name))
            .with_context(|| format!("unknown building kind '{name}'"))?;
        garrison.push((kind, count.trim().parse()?));
    }
    Ok(garrison)
}

/// Total scrap a garrison costs.
pub fn garrison_cost(garrison: &Garrison) -> u32 {
    garrison
        .iter()
        .map(|(kind, n)| structure_cost(*kind) * n)
        .sum()
}

fn structure_cost(kind: BuildingKind) -> u32 {
    kind.stats().construction.map(|c| c.cost).unwrap_or(0)
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
    siege(a, b, &[], seed, max_ticks)
}

/// A duel where side B also holds ground with pre-built structures.
/// The garrison stands in a pitch-3 grid ahead of B's deployment, its
/// purchase value counts toward B, and B may field no units at all —
/// pure fortification is a legitimate experiment.
pub fn siege(
    a: &[(UnitKind, u32)],
    b: &[(UnitKind, u32)],
    garrison: &[(BuildingKind, u32)],
    seed: u64,
    max_ticks: u64,
) -> Result<DuelOutcome> {
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
                } else if (x, y) == (width - 4, height - 4) {
                    // The exact 180-degree image of the 2x2 anchor at
                    // (2,2): top-left maps to (W-2-x, H-2-y).
                    '2'
                } else {
                    '.'
                }
            })
            .collect();
        map.push(row);
    }
    let mut units = Vec::new();
    // Side B's k-th unit is the exact 180-degree image of side A's
    // k-th, entry by entry — the repo's seat-fairness rule. Anything
    // less contaminates a controlled duel with seat geometry.
    let mut place = |army: &[(UnitKind, u32)], player: u8, mirrored: bool| {
        let mut i = 0i32;
        for (kind, n) in army {
            for _ in 0..*n {
                let (x, y) = (8 + (i / 16), 4 + (i % 16));
                let (x, y) = if mirrored {
                    (width - 1 - x, height - 1 - y)
                } else {
                    (x, y)
                };
                units.push(UnitSpec {
                    player,
                    kind: *kind,
                    x,
                    y,
                });
                i += 1;
            }
        }
    };
    place(a, 0, false);
    place(b, 1, true);
    // The garrison's pitch-3 grid (2x2 kinds fit) walks toward side A
    // one column at a time, clear of B's unit columns at x 30-31 and
    // its foundry at (36,20).
    let mut buildings = Vec::new();
    let mut g = 0i32;
    for (kind, n) in garrison {
        for _ in 0..*n {
            if g >= 18 {
                bail!("garrison caps at 18 structures");
            }
            buildings.push(BuildingSpec {
                player: 1,
                kind: *kind,
                x: 27 - 3 * (g / 6),
                y: 4 + 3 * (g % 6),
            });
            g += 1;
        }
    }
    let scenario = Scenario {
        name: "arena-duel".into(),
        seed,
        map,
        players: vec![seat("A", Faction::Ferrous), seat("B", Faction::Cupric)],
        units,
        buildings,
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
    if a_ids.is_empty() || (b_ids.is_empty() && state.buildings().len() <= 2) {
        bail!("side A needs units; side B needs units or a garrison");
    }
    let mut opening = vec![PlayerCommand {
        player: PlayerId(0),
        command: Command::AttackMove {
            units: a_ids,
            goal: TilePos::new(33, 12),
            queue: false,
        },
    }];
    if !b_ids.is_empty() {
        opening.push(PlayerCommand {
            player: PlayerId(1),
            command: Command::AttackMove {
                units: b_ids,
                // The exact image of side A's goal.
                goal: TilePos::new(40 - 1 - 33, 24 - 1 - 12),
                queue: false,
            },
        });
    }
    state.tick(&opening);
    // Every accepted unit carries its purchase value — a harvester
    // screen is a legitimate experiment, and valuing it at zero once
    // declared a live side wiped after two ticks. Garrison structures
    // count the same way; the arena Foundries never do (they are the
    // victory tokens, not purchases).
    let value = |state: &oxide_sim::State, player: u8| -> u32 {
        let units: u32 = state
            .units()
            .iter()
            .filter(|u| u.player == PlayerId(player))
            .map(|u| u.kind.stats().cost)
            .sum();
        let structures: u32 = state
            .buildings()
            .iter()
            .filter(|b| b.player == PlayerId(player) && b.kind != BuildingKind::Foundry)
            .map(|b| structure_cost(b.kind))
            .sum();
        units + structures
    };
    let hp_sum = |state: &oxide_sim::State, player: u8| -> u64 {
        let units: u64 = state
            .units()
            .iter()
            .filter(|u| u.player == PlayerId(player))
            .map(|u| u64::from(u.hp))
            .sum();
        let structures: u64 = state
            .buildings()
            .iter()
            .filter(|b| b.player == PlayerId(player) && b.kind != BuildingKind::Foundry)
            .map(|b| u64::from(b.hp))
            .sum();
        units + structures
    };
    let mut last = (value(&state, 0), value(&state, 1));
    let mut last_hp = (hp_sum(&state, 0), hp_sum(&state, 1));
    let mut combat_started = false;
    let mut quiet = 0u64;
    let mut ran = 1;
    for _ in 1..max_ticks {
        state.tick(&[]);
        ran += 1;
        let now = (value(&state, 0), value(&state, 1));
        let now_hp = (hp_sum(&state, 0), hp_sum(&state, 1));
        if now_hp.0 < last_hp.0 || now_hp.1 < last_hp.1 {
            combat_started = true;
        }
        // Quiet means no combat progress — hp and value both frozen.
        // Value alone stayed flat through whole approach marches and
        // nonlethal exchanges, ending slow matchups as phantom draws.
        if now == last && now_hp == last_hp {
            quiet += 1
        } else {
            quiet = 0
        }
        last = now;
        last_hp = now_hp;
        // One side wiped, or — once battle has actually been joined —
        // nothing has changed for 15 seconds of sim time. Before first
        // blood the armies are still marching; a duel where contact
        // never comes runs to the cap and says so via `ticks`.
        if now.0 == 0 || now.1 == 0 || (combat_started && quiet > 300) {
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

    #[test]
    fn a_slow_duel_resolves_instead_of_ending_as_a_phantom_draw() {
        // Two lone bombards spend hundreds of ticks marching before a
        // shell flies. Value-only quiescence once ended this at tick
        // 302 with both sides reported intact; combat-gated quiescence
        // must let the duel actually resolve.
        let army = parse_army("bombard:1").unwrap();
        let out = duel(&army, &army, 42, 20_000).unwrap();
        assert!(
            out.a_value == 0 || out.b_value == 0 || out.ticks == 20_000,
            "the duel neither resolved nor ran honestly to the cap: {out:?}"
        );
        assert!(out.ticks > 302, "ended during the approach: {out:?}");
    }

    #[test]
    fn the_garrison_parser_speaks_building_names() {
        let garrison = parse_garrison("turret:2, FlakTurret:1").unwrap();
        assert_eq!(garrison.len(), 2);
        assert_eq!(
            garrison_cost(&garrison),
            2 * structure_cost(BuildingKind::Turret) + structure_cost(BuildingKind::FlakTurret)
        );
        assert!(
            parse_garrison("foundry:1").is_err(),
            "victory tokens refused"
        );
        assert!(parse_garrison("keep:1").is_err());
    }

    #[test]
    fn a_lone_raider_breaks_on_a_fortified_line() {
        // Defense mode's floor: one scuttler cannot crack two turrets,
        // and a unit-less defending side must not read as pre-wiped.
        let raiders = parse_army("scuttler:1").unwrap();
        let garrison = parse_garrison("turret:2").unwrap();
        let out = siege(&raiders, &[], &garrison, 42, 8_000).unwrap();
        assert_eq!(out.a_value, 0, "the raider dies on the wall: {out:?}");
        assert!(
            out.b_value >= garrison_cost(&garrison),
            "the standing wall keeps its purchase value: {out:?}"
        );
    }

    #[test]
    fn harvesters_carry_their_purchase_value() {
        // A harvester screen is a legitimate experiment; valuing the
        // workers at zero once declared a live side wiped on the spot.
        let workers = parse_army("harvester:4").unwrap();
        let fighters = parse_army("sentinel:1").unwrap();
        let out = duel(&fighters, &workers, 42, 4_000).unwrap();
        let worker_cost = army_cost(&workers);
        assert!(
            out.b_value > 0 || out.ticks > 50,
            "a live harvester side must not read as instantly wiped: {out:?} (cost {worker_cost})"
        );
    }
}
