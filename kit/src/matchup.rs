//! Paired arena duels: two hand-picked armies on flat ground, no economy,
//! attack-moved into each other — a controlled counter experiment. The
//! reported signal is surviving purchase value: useful when callers choose
//! comparable starting budgets, but neither cost equality nor remaining HP
//! is folded into the verdict.
//!
//! Defense mode stands pre-built structures in front of side B: the
//! swarm-vs-fortification experiment. The roles are deliberately
//! asymmetric, but the paired run still exchanges their physical seats;
//! the garrison travels with B and counts toward its verdict like any
//! purchase.

use anyhow::{Context, Result, bail};
use chassis::grid::TilePos;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::{BuildingKind, Command, Faction, PlayerCommand, PlayerId, Scenario, UnitKind};
use std::fmt;

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

/// One physical orientation of a duel: surviving purchase value per
/// logical side after the dust settles (zero on both sides is mutual
/// annihilation).
#[derive(Debug, Clone, serde::Serialize)]
pub struct DuelLegOutcome {
    /// The player/seat occupied by logical side A in this leg.
    pub a_player: u8,
    /// Side A's surviving purchase value.
    pub a_value: u32,
    /// Side B's surviving purchase value.
    pub b_value: u32,
    /// Ticks elapsed before the termination condition or cap.
    pub ticks: u64,
    /// Why this leg stopped.
    pub termination: DuelTermination,
}

/// Why one physical leg stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DuelTermination {
    /// At least one side lost every priced unit and structure.
    Wipe,
    /// Combat began, then HP and survivor value stopped changing for
    /// the harness's 301-tick observation window.
    NoProgress,
    /// The requested tick limit was reached.
    Cap,
}

impl fmt::Display for DuelTermination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Wipe => "wipe",
            Self::NoProgress => "no-progress",
            Self::Cap => "cap",
        })
    }
}

/// Winner of one leg or of the seat-neutral paired aggregate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DuelVerdict {
    /// Logical side A has more surviving value.
    A,
    /// Logical side B has more surviving value.
    B,
    /// Both sides have equal surviving value.
    Tie,
}

impl fmt::Display for DuelVerdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::A => "A",
            Self::B => "B",
            Self::Tie => "tie",
        })
    }
}

impl DuelLegOutcome {
    /// Verdict from this leg's surviving purchase values. No-progress and
    /// capped matches are unresolved, whatever happened to be alive when
    /// the instrument stopped.
    pub fn verdict(&self) -> Option<DuelVerdict> {
        (self.termination == DuelTermination::Wipe)
            .then(|| verdict(u64::from(self.a_value), u64::from(self.b_value)))
    }
}

/// Seat-neutral result of a duel: the same logical armies play once with
/// A as player 0 and once with A as player 1. Keeping both legs visible
/// exposes deterministic seat/ID effects instead of averaging them away.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DuelOutcome {
    /// A occupies player 0 (west, and lower initial unit IDs).
    pub a_as_player_0: DuelLegOutcome,
    /// A occupies player 1 (east, and higher initial unit IDs).
    pub a_as_player_1: DuelLegOutcome,
}

impl DuelOutcome {
    /// Side A's surviving value summed across both orientations.
    pub fn a_total_value(&self) -> u64 {
        u64::from(self.a_as_player_0.a_value) + u64::from(self.a_as_player_1.a_value)
    }

    /// Side B's surviving value summed across both orientations.
    pub fn b_total_value(&self) -> u64 {
        u64::from(self.a_as_player_0.b_value) + u64::from(self.a_as_player_1.b_value)
    }

    /// Side A's seat-neutral mean surviving value.
    pub fn a_mean_value(&self) -> f64 {
        self.a_total_value() as f64 / 2.0
    }

    /// Side B's seat-neutral mean surviving value.
    pub fn b_mean_value(&self) -> f64 {
        self.b_total_value() as f64 / 2.0
    }

    /// Verdict after both orientations receive equal weight. If either leg
    /// stopped making progress or hit the cap, the pair is unresolved.
    pub fn verdict(&self) -> Option<DuelVerdict> {
        self.legs()
            .iter()
            .all(|leg| leg.termination == DuelTermination::Wipe)
            .then(|| verdict(self.a_total_value(), self.b_total_value()))
    }

    /// Whether the winner changes when the armies exchange seats/ID
    /// ranges. Unresolved legs make the comparison unknown.
    pub fn verdict_flips_on_swap(&self) -> Option<bool> {
        Some(self.a_as_player_0.verdict()? != self.a_as_player_1.verdict()?)
    }

    /// Both physical legs, in player-0 then player-1 order for side A.
    pub fn legs(&self) -> [&DuelLegOutcome; 2] {
        [&self.a_as_player_0, &self.a_as_player_1]
    }
}

fn verdict(a_value: u64, b_value: u64) -> DuelVerdict {
    match a_value.cmp(&b_value) {
        std::cmp::Ordering::Greater => DuelVerdict::A,
        std::cmp::Ordering::Less => DuelVerdict::B,
        std::cmp::Ordering::Equal => DuelVerdict::Tie,
    }
}

/// Runs a seat-neutral duel on an open arena: armies deploy in mirrored
/// lines and attack-move through each other's positions, then exchange
/// seats and initial ID ranges for the second leg.
pub fn duel(a: &Army, b: &Army, seed: u64, max_ticks: u64) -> Result<DuelOutcome> {
    siege(a, b, &[], seed, max_ticks)
}

/// A seat-neutral duel where side B also holds ground with pre-built
/// structures. The garrison travels with B when the armies exchange
/// seats, stands in a pitch-3 grid ahead of B's deployment, and counts
/// toward B's surviving purchase value. B may field no units at all —
/// pure fortification is a legitimate experiment.
pub fn siege(
    a: &[(UnitKind, u32)],
    b: &[(UnitKind, u32)],
    garrison: &[(BuildingKind, u32)],
    seed: u64,
    max_ticks: u64,
) -> Result<DuelOutcome> {
    Ok(DuelOutcome {
        a_as_player_0: siege_leg(a, b, garrison, seed, max_ticks, 0)?,
        a_as_player_1: siege_leg(a, b, garrison, seed, max_ticks, 1)?,
    })
}

/// One physical leg. Logical side A occupies `a_player`; the other
/// logical side and its optional garrison occupy the opposite seat.
fn siege_leg(
    a: &[(UnitKind, u32)],
    b: &[(UnitKind, u32)],
    garrison: &[(BuildingKind, u32)],
    seed: u64,
    max_ticks: u64,
    a_player: u8,
) -> Result<DuelLegOutcome> {
    anyhow::ensure!(a_player <= 1, "arena has only players 0 and 1");
    anyhow::ensure!(max_ticks > 0, "tick cap must be greater than zero");
    let b_player = 1 - a_player;
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
    // The east side's k-th deployment slot is the exact 180-degree
    // image of the west side's k-th slot. Anything less contaminates a
    // controlled duel with seat geometry.
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
    if a_player == 0 {
        place(a, 0, false);
        place(b, 1, true);
    } else {
        // Scenario order assigns the initial entity IDs. In the return
        // leg B therefore receives both player 0 and the lower ID range.
        place(b, 0, false);
        place(a, 1, true);
    }
    // The garrison's pitch-3 grid (2x2 kinds fit) walks toward side A
    // one column at a time, clear of the east unit columns at x 30-31
    // and its foundry at (36,20). Its west orientation is the exact
    // 180-degree image, adjusted for each structure's footprint.
    let mut buildings = Vec::new();
    let mut g = 0i32;
    for (kind, n) in garrison {
        for _ in 0..*n {
            if g >= 18 {
                bail!("garrison caps at 18 structures");
            }
            let east_x = 27 - 3 * (g / 6);
            let east_y = 4 + 3 * (g % 6);
            let (x, y) = if b_player == 1 {
                (east_x, east_y)
            } else {
                let (building_width, building_height) = kind.stats().size;
                (
                    width - building_width - east_x,
                    height - building_height - east_y,
                )
            };
            buildings.push(BuildingSpec {
                player: b_player,
                kind: *kind,
                x,
                y,
            });
            g += 1;
        }
    }
    let scenario = Scenario {
        name: "arena-duel".into(),
        seed,
        map,
        players: vec![
            seat("West", Faction::Ferrous),
            seat("East", Faction::Cupric),
        ],
        units,
        buildings,
        meta: None,
    };
    let mut state = scenario.build().context("arena builds")?;
    let (player_0_ids, player_1_ids): (Vec<_>, Vec<_>) = {
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
    let (a_ids, b_ids) = if a_player == 0 {
        (&player_0_ids, &player_1_ids)
    } else {
        (&player_1_ids, &player_0_ids)
    };
    let b_has_garrison = state.buildings().iter().any(|building| {
        building.player == PlayerId(b_player) && building.kind != BuildingKind::Foundry
    });
    if a_ids.is_empty() || (b_ids.is_empty() && !b_has_garrison) {
        bail!("side A needs units; side B needs units or a garrison");
    }
    let mut opening = Vec::with_capacity(2);
    if !player_0_ids.is_empty() {
        opening.push(PlayerCommand {
            player: PlayerId(0),
            command: Command::AttackMove {
                units: player_0_ids,
                goal: TilePos::new(33, 12),
                queue: false,
            },
        });
    }
    if !player_1_ids.is_empty() {
        opening.push(PlayerCommand {
            player: PlayerId(1),
            command: Command::AttackMove {
                units: player_1_ids,
                // The exact image of player 0's goal.
                goal: TilePos::new(width - 1 - 33, height - 1 - 12),
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
    let mut last = (value(&state, a_player), value(&state, b_player));
    let mut last_hp = (hp_sum(&state, a_player), hp_sum(&state, b_player));
    let mut combat_started = false;
    let mut quiet = 0u64;
    let mut ran = 1;
    let mut termination = if last.0 == 0 || last.1 == 0 {
        DuelTermination::Wipe
    } else {
        DuelTermination::Cap
    };
    for _ in 1..max_ticks {
        if termination == DuelTermination::Wipe {
            break;
        }
        state.tick(&[]);
        ran += 1;
        let now = (value(&state, a_player), value(&state, b_player));
        let now_hp = (hp_sum(&state, a_player), hp_sum(&state, b_player));
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
        if now.0 == 0 || now.1 == 0 {
            termination = DuelTermination::Wipe;
            break;
        }
        if combat_started && quiet > 300 {
            termination = DuelTermination::NoProgress;
            break;
        }
    }
    Ok(DuelLegOutcome {
        a_player,
        a_value: last.0,
        b_value: last.1,
        ticks: ran,
        termination,
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
    fn mirrored_armies_are_exactly_neutral_across_both_orientations() {
        let army = parse_army("sentinel:6").unwrap();
        let out = duel(&army, &army, 42, 6_000).unwrap();

        assert_eq!(
            out.a_as_player_0.a_value, out.a_as_player_1.b_value,
            "exchanging identical armies must exchange the physical result: {out:?}"
        );
        assert_eq!(
            out.a_as_player_0.b_value, out.a_as_player_1.a_value,
            "exchanging identical armies must exchange the physical result: {out:?}"
        );
        assert_eq!(
            out.a_total_value(),
            out.b_total_value(),
            "the paired mirror must be exactly neutral: {out:?}"
        );
        assert_eq!(out.verdict(), Some(DuelVerdict::Tie));
    }

    #[test]
    fn orientation_dependent_matchups_report_both_verdicts() {
        // This pairing exposes a deterministic orientation/player-order
        // effect. A single leg declared opposite winners depending only
        // on which physical side the logical army occupied.
        let siege_line = parse_army("bombard:5,scuttler:5").unwrap();
        let sentinels = parse_army("sentinel:13").unwrap();
        let out = duel(&siege_line, &sentinels, 42, 8_000).unwrap();

        // These two assertions pin a MEASURED orientation effect under the
        // current balance numbers. A stats or movement bless can
        // legitimately flip a leg; if one fails after such a change,
        // re-measure and update the pinned winners rather than suspecting
        // the pairing machinery.
        assert_eq!(out.a_as_player_0.verdict(), Some(DuelVerdict::B), "{out:?}");
        assert_eq!(out.a_as_player_1.verdict(), Some(DuelVerdict::A), "{out:?}");
        assert_eq!(
            out.verdict_flips_on_swap(),
            Some(true),
            "the paired result must surface a seat-dependent winner: {out:?}"
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
        for leg in out.legs() {
            assert!(
                leg.a_value == 0 || leg.b_value == 0 || leg.ticks == 20_000,
                "the duel neither resolved nor ran honestly to the cap: {out:?}"
            );
            assert!(leg.ticks > 302, "ended during the approach: {out:?}");
        }
    }

    #[test]
    fn a_capped_pair_is_unresolved_regardless_of_survivor_value() {
        let a = parse_army("sentinel:2").unwrap();
        let b = parse_army("scuttler:2").unwrap();
        let out = duel(&a, &b, 42, 1).unwrap();

        for leg in out.legs() {
            assert_eq!(leg.termination, DuelTermination::Cap, "{out:?}");
            assert_eq!(leg.verdict(), None, "{out:?}");
        }
        assert_eq!(out.verdict(), None);
        assert_eq!(out.verdict_flips_on_swap(), None);
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
        for leg in out.legs() {
            assert_eq!(leg.a_value, 0, "the raider dies on the wall: {out:?}");
            assert!(
                leg.b_value >= garrison_cost(&garrison),
                "the standing wall keeps its purchase value: {out:?}"
            );
        }
        assert_eq!(out.verdict(), Some(DuelVerdict::B));
    }

    #[test]
    fn harvesters_carry_their_purchase_value() {
        // A harvester screen is a legitimate experiment; valuing the
        // workers at zero once declared a live side wiped on the spot.
        let workers = parse_army("harvester:4").unwrap();
        let fighters = parse_army("sentinel:1").unwrap();
        let out = duel(&fighters, &workers, 42, 4_000).unwrap();
        let worker_cost = army_cost(&workers);
        for leg in out.legs() {
            assert!(
                leg.b_value > 0 || leg.ticks > 50,
                "a live harvester side must not read as instantly wiped: {out:?} (cost \
                 {worker_cost})"
            );
        }
    }
}
