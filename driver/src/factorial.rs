//! The factorial fairness probe: every advantage the shipped game binds
//! to the seat index, permuted one lever at a time and all together.
//!
//! `sweep` seats the same commander in both chairs and reads the
//! aggregate lean; this probe unbundles what the chair itself carries:
//! which roster a seat plays, which end of the map it starts from,
//! which id range its starting units claim, and where its commands land
//! in the tick's command slice. Each is a factor with its own levels;
//! the design is their full cross product, every cell played on the
//! same seed set.
//!
//! The response is seat 0's win rate over decided matches, reported per
//! factor level with a 95% Wilson interval, alongside decision-tick
//! quartiles and the censored share. Marginals alone would lie here —
//! the same-roster mirrors are known to lean in *opposite* directions,
//! which an average erases — so the full cell table is part of the
//! report, not an appendix.
//!
//! Nothing in the probe changes the sim: every cell is a transform of
//! the scenario or of how the harness assembles the tick, and the
//! all-baseline cell reproduces a direct Overseer-vs-Overseer run bit
//! for bit (a test pins that against the sim stepped by hand).

use crate::sweep::SweepOutcome;
use anyhow::{Context, Result};
use oxide_sim::bot::Brain;
use oxide_sim::scenario::Scenario;
use oxide_sim::{BuildingKind, Faction, GameResult, PlayerId, State};
use serde::Serialize;

/// How many levers the design carries.
pub const FACTOR_COUNT: usize = 4;

/// One lever of the design. Every factor is something the shipped game
/// ties to the seat number; a marginal that moves when the lever flips
/// is an edge the chair carries, not the player sitting in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Factor {
    /// Which roster each seat plays, all four combinations.
    Faction,
    /// Which seat's starting units claim the low unit-id range.
    Spawn,
    /// Which seat's commands land first in the tick's command slice.
    Command,
    /// Whether the map is played as authored or rotated 180 degrees, so
    /// a player index changes ends without changing terrain.
    Geometry,
}

impl Factor {
    /// Every factor, in report order.
    pub const ALL: [Factor; FACTOR_COUNT] = [
        Factor::Faction,
        Factor::Spawn,
        Factor::Command,
        Factor::Geometry,
    ];

    /// The `--factors` key.
    pub fn key(self) -> &'static str {
        match self {
            Factor::Faction => "faction",
            Factor::Spawn => "spawn",
            Factor::Command => "command",
            Factor::Geometry => "geometry",
        }
    }

    /// This factor's levels, baseline first.
    pub fn levels(self) -> &'static [&'static str] {
        match self {
            // First letter is seat 0's roster.
            Factor::Faction => &["FC", "CF", "FF", "CC"],
            Factor::Spawn => &["seat0-first", "seat1-first"],
            Factor::Command => &["seat0-first", "seat1-first"],
            Factor::Geometry => &["authored", "rot180"],
        }
    }

    /// Position in [`Factor::ALL`], which is also the cell index.
    pub fn index(self) -> usize {
        Factor::ALL
            .iter()
            .position(|f| *f == self)
            .expect("every factor is in ALL")
    }

    /// Resolves a `--factors` key.
    pub fn parse(key: &str) -> Result<Factor> {
        Factor::ALL
            .iter()
            .copied()
            .find(|f| f.key() == key)
            .with_context(|| {
                let keys: Vec<&str> = Factor::ALL.iter().map(|f| f.key()).collect();
                format!("unknown factor {key:?}; known factors: {}", keys.join(", "))
            })
    }
}

/// Level indices, one per factor, in [`Factor::ALL`] order. A disabled
/// factor stays pinned at level 0.
pub type Cell = [u8; FACTOR_COUNT];

/// The roster's report name.
fn roster(faction: Faction) -> &'static str {
    match faction {
        Faction::Ferrous => "ferrous",
        Faction::Cupric => "cupric",
    }
}

/// Seat rosters per [`Factor::Faction`] level.
const FACTION_CELLS: [[Faction; 2]; 4] = [
    [Faction::Ferrous, Faction::Cupric],
    [Faction::Cupric, Faction::Ferrous],
    [Faction::Ferrous, Faction::Ferrous],
    [Faction::Cupric, Faction::Cupric],
];

/// One match of the design.
#[derive(Debug, Clone, Serialize)]
pub struct FactorialMatch {
    /// Scenario seed this match ran under.
    pub seed: u64,
    /// Level index per factor, in [`Factor::ALL`] order.
    pub cell: Vec<u8>,
    /// The same cell as level labels, enabled factors only.
    pub levels: Vec<String>,
    /// The roster each seat actually played.
    pub factions: [String; 2],
    /// Final tick: the decision tick, or the cap.
    pub ticks: u64,
    /// How it ended.
    pub outcome: SweepOutcome,
    /// Final state hash — the handle that reproduces this exact match.
    pub hash: u64,
}

/// Decision-tick quartiles over a set of decided matches.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Quartiles {
    /// Lower quartile.
    pub p25: u64,
    /// Median.
    pub median: u64,
    /// Upper quartile.
    pub p75: u64,
}

/// One level of one factor, folded over every match that played it.
#[derive(Debug, Clone, Serialize)]
pub struct LevelRecord {
    /// The level's label.
    pub level: String,
    /// Matches played at this level.
    pub matches: u32,
    /// Matches a seat won outright.
    pub victories: u32,
    /// Mutual-death draws.
    pub draws: u32,
    /// Matches that hit the cap.
    pub undecided: u32,
    /// Undecided share of the level's matches, in percent.
    pub censored_percent: f64,
    /// Victories by seat.
    pub seat_wins: [u32; 2],
    /// Seat 0's share of the victories.
    pub seat0_win_rate: Option<f64>,
    /// 95% Wilson score interval on that share.
    pub wilson: Option<[f64; 2]>,
    /// Decision-tick quartiles over decided matches.
    pub decision_ticks: Option<Quartiles>,
}

/// A factor and every level of it.
#[derive(Debug, Clone, Serialize)]
pub struct FactorRecord {
    /// The factor's key.
    pub factor: String,
    /// Its levels, baseline first.
    pub per_level: Vec<LevelRecord>,
}

/// One cell of the design — the row that shows interactions a marginal
/// would average away.
#[derive(Debug, Clone, Serialize)]
pub struct CellRecord {
    /// Level labels, enabled factors only, in [`Factor::ALL`] order.
    pub levels: Vec<String>,
    /// Matches played in this cell (one per seed).
    pub matches: u32,
    /// Victories by seat.
    pub seat_wins: [u32; 2],
    /// Mutual-death draws.
    pub draws: u32,
    /// Matches that hit the cap.
    pub undecided: u32,
    /// Undecided share of the cell's matches, in percent.
    pub censored_percent: f64,
    /// Median decision tick over decided matches.
    pub median_decision_tick: Option<u64>,
}

/// The roster marginal, read over the cells where the two seats play
/// *different* rosters — the mirrors carry no roster information.
#[derive(Debug, Clone, Serialize)]
pub struct RosterRecord {
    /// Mixed-roster victories won by a Ferrous seat.
    pub ferrous_wins: u32,
    /// Mixed-roster victories won by a Cupric seat.
    pub cupric_wins: u32,
    /// Ferrous share of those victories.
    pub ferrous_win_rate: f64,
    /// 95% Wilson score interval on that share.
    pub wilson: [f64; 2],
}

/// The probe's verdict.
#[derive(Debug, Clone, Serialize)]
pub struct FactorialReport {
    /// Scenario name.
    pub scenario: String,
    /// Seeds each cell was played on.
    pub seeds: u64,
    /// First scenario seed.
    pub seed_base: u64,
    /// Tick cap per match.
    pub max_ticks: u64,
    /// Enabled factor keys.
    pub factors: Vec<String>,
    /// Cells in the design.
    pub cells: usize,
    /// Matches played.
    pub matches_played: u32,
    /// Matches a seat won outright.
    pub victories: u32,
    /// Mutual-death draws.
    pub draws: u32,
    /// Matches that hit the cap.
    pub undecided: u32,
    /// Undecided share of every match, in percent.
    pub censored_percent: f64,
    /// Per-factor marginals.
    pub per_factor: Vec<FactorRecord>,
    /// The roster marginal, when any mixed-roster cell was played.
    pub roster: Option<RosterRecord>,
    /// Every cell of the design.
    pub per_cell: Vec<CellRecord>,
    /// Every match, in cell-then-seed order.
    pub matches: Vec<FactorialMatch>,
}

/// Rotates a scenario 180 degrees: terrain, Foundry anchors, starting
/// units and pre-built structures all turn together, so seat N keeps
/// its player index while playing the geometry its mirror held. That is
/// the only way to unbundle player index from map position.
///
/// Refuses a map whose terrain is not exactly 180-symmetric with the
/// anchor digits read as ground — rotating one would hand the seats
/// different worlds, and every verdict after it would be about the
/// terrain instead of the seat.
pub fn rotate_180(base: &Scenario) -> Result<Scenario> {
    let height = base.map.len();
    anyhow::ensure!(height > 0, "{} has an empty map", base.name);
    let rows: Vec<Vec<char>> = base.map.iter().map(|r| r.chars().collect()).collect();
    let width = rows[0].len();
    anyhow::ensure!(
        rows.iter().all(|r| r.len() == width),
        "{}'s map is ragged; the rotation needs a rectangle",
        base.name
    );
    let is_anchor = |c: char| c.is_ascii_digit() && c != '0' && c != '9';
    let ground = |c: char| if is_anchor(c) { '.' } else { c };
    for (y, row) in rows.iter().enumerate() {
        for (x, &c) in row.iter().enumerate() {
            anyhow::ensure!(
                ground(c) == ground(rows[height - 1 - y][width - 1 - x]),
                "{} is not 180-symmetric at ({x}, {y}) — the probe will not rotate it",
                base.name
            );
        }
    }

    let mut map: Vec<Vec<char>> = (0..height)
        .map(|y| {
            (0..width)
                .map(|x| ground(rows[height - 1 - y][width - 1 - x]))
                .collect()
        })
        .collect();
    // An anchor digit names the TOP-LEFT of the Foundry's footprint, so
    // its rotation lands a footprint in from the rotated corner. That
    // target sits inside the original footprint and is therefore open
    // ground the symmetry check already cleared.
    let (fw, fh) = BuildingKind::Foundry.base_stats().size;
    let (fw, fh) = (fw as usize, fh as usize);
    for (y, row) in rows.iter().enumerate() {
        for (x, &c) in row.iter().enumerate() {
            if !is_anchor(c) {
                continue;
            }
            let (Some(ty), Some(tx)) = (
                height.checked_sub(fh).and_then(|h| h.checked_sub(y)),
                width.checked_sub(fw).and_then(|w| w.checked_sub(x)),
            ) else {
                anyhow::bail!(
                    "{}'s anchor at ({x}, {y}) has no room for a Foundry",
                    base.name
                );
            };
            map[ty][tx] = c;
        }
    }

    let mut out = base.clone();
    out.map = map.into_iter().map(|r| r.into_iter().collect()).collect();
    let (w, h) = (width as i32, height as i32);
    for unit in &mut out.units {
        unit.x = w - 1 - unit.x;
        unit.y = h - 1 - unit.y;
    }
    for building in &mut out.buildings {
        let (bw, bh) = building.kind.base_stats().size;
        building.x = w - building.x - bw;
        building.y = h - building.y - bh;
    }
    Ok(out)
}

/// Stable-partitions the starting units so `first`'s specs claim the
/// low id range. Ids are handed out by `Scenario::build`'s walk over
/// this vector and nothing else reads its order, which makes the
/// partition the id-range lever on its own.
///
/// Both levels of the factor re-group the list, so on a map whose
/// authored list interleaves the seats neither level reproduces the
/// authored ids — the comparison stays controlled, which is the point.
pub fn permute_spawn_order(scenario: &mut Scenario, first: u8) {
    let (mut head, tail): (Vec<_>, Vec<_>) = scenario
        .units
        .iter()
        .copied()
        .partition(|u| u.player == first);
    head.extend(tail);
    scenario.units = head;
}

/// The design's cells: the full cross product of the enabled factors'
/// levels, with every disabled factor pinned at level 0.
fn design(enabled: &[Factor]) -> Vec<Cell> {
    let mut cells = vec![[0u8; FACTOR_COUNT]];
    for factor in enabled {
        let mut next = Vec::with_capacity(cells.len() * factor.levels().len());
        for cell in &cells {
            for level in 0..factor.levels().len() as u8 {
                let mut grown = *cell;
                grown[factor.index()] = level;
                next.push(grown);
            }
        }
        cells = next;
    }
    cells
}

/// Level labels for the enabled factors, in [`Factor::ALL`] order.
fn labels(enabled: &[Factor], cell: &Cell) -> Vec<String> {
    Factor::ALL
        .iter()
        .filter(|f| enabled.contains(f))
        .map(|f| f.levels()[cell[f.index()] as usize].to_string())
        .collect()
}

/// Runs the design headless and returns the verdict. Cells fan out
/// across a worker pool; every match is an independent deterministic
/// sim, so the report is a function of the design and the seed set.
pub fn run_factorial(
    scenario: &str,
    enabled: &[Factor],
    seeds: u64,
    max_ticks: u64,
    seed_base: u64,
) -> Result<FactorialReport> {
    anyhow::ensure!(!enabled.is_empty(), "the design needs at least one factor");
    anyhow::ensure!(
        enabled
            .iter()
            .enumerate()
            .all(|(i, f)| !enabled[..i].contains(f)),
        "a factor may appear in the design only once"
    );
    // Normalized to report order: the cell table's columns and its rows
    // are both read off `Factor::ALL`, and a caller-ordered list would
    // label them apart.
    let enabled: Vec<Factor> = Factor::ALL
        .iter()
        .copied()
        .filter(|f| enabled.contains(f))
        .collect();
    let enabled = enabled.as_slice();
    let base = crate::runner::load_scenario(scenario)?;
    anyhow::ensure!(
        base.players.len() == 2,
        "the factorial probe reads 1v1 fairness; {} has {} seats",
        base.name,
        base.players.len()
    );
    // Refuse up front rather than one worker at a time, so an
    // unrotatable map costs no simulation at all.
    if enabled.contains(&Factor::Geometry) {
        rotate_180(&base)?;
    }

    let cells = design(enabled);
    let jobs: Vec<(Cell, u64)> = cells
        .iter()
        .flat_map(|cell| (0..seeds).map(move |offset| (*cell, seed_base + offset)))
        .collect();
    let matches = crate::pool::fan_out(&jobs, |&(cell, seed)| {
        let played = play(&base, seed, cell, max_ticks)?;
        eprintln!(
            "  {} · seed {} · {} ticks · {:?}",
            labels(enabled, &cell).join(" "),
            played.seed,
            played.ticks,
            played.outcome
        );
        Ok(FactorialMatch {
            levels: labels(enabled, &cell),
            ..played
        })
    })?;

    let per_factor = enabled
        .iter()
        .map(|factor| FactorRecord {
            factor: factor.key().to_string(),
            per_level: factor
                .levels()
                .iter()
                .enumerate()
                .map(|(level, label)| {
                    let played: Vec<&FactorialMatch> = matches
                        .iter()
                        .filter(|m| usize::from(m.cell[factor.index()]) == level)
                        .collect();
                    level_record(label, &played)
                })
                .collect(),
        })
        .collect();

    let per_cell = cells
        .iter()
        .map(|cell| {
            let played: Vec<&FactorialMatch> = matches
                .iter()
                .filter(|m| m.cell == cell.as_slice())
                .collect();
            let tally = Tally::of(&played);
            CellRecord {
                levels: labels(enabled, cell),
                matches: tally.matches,
                seat_wins: tally.seat_wins,
                draws: tally.draws,
                undecided: tally.undecided,
                censored_percent: tally.censored_percent(),
                median_decision_tick: tally.quartiles().map(|q| q.median),
            }
        })
        .collect();

    let overall = Tally::of(&matches.iter().collect::<Vec<_>>());
    Ok(FactorialReport {
        scenario: base.name,
        seeds,
        seed_base,
        max_ticks,
        factors: enabled.iter().map(|f| f.key().to_string()).collect(),
        cells: cells.len(),
        matches_played: overall.matches,
        victories: overall.seat_wins[0] + overall.seat_wins[1],
        draws: overall.draws,
        undecided: overall.undecided,
        censored_percent: overall.censored_percent(),
        per_factor,
        roster: roster_record(&matches),
        per_cell,
        matches,
    })
}

/// The fold every record is read off: outcomes counted, decision ticks
/// collected.
struct Tally {
    matches: u32,
    seat_wins: [u32; 2],
    draws: u32,
    undecided: u32,
    decision_ticks: Vec<u64>,
}

impl Tally {
    fn of(played: &[&FactorialMatch]) -> Self {
        let mut tally = Tally {
            matches: 0,
            seat_wins: [0; 2],
            draws: 0,
            undecided: 0,
            decision_ticks: Vec::new(),
        };
        for m in played {
            tally.matches += 1;
            match m.outcome {
                SweepOutcome::Victory { seat } => {
                    tally.seat_wins[usize::from(seat)] += 1;
                    tally.decision_ticks.push(m.ticks);
                }
                SweepOutcome::Draw => {
                    tally.draws += 1;
                    tally.decision_ticks.push(m.ticks);
                }
                SweepOutcome::Undecided => tally.undecided += 1,
            }
        }
        tally.decision_ticks.sort_unstable();
        tally
    }

    fn censored_percent(&self) -> f64 {
        if self.matches == 0 {
            0.0
        } else {
            100.0 * f64::from(self.undecided) / f64::from(self.matches)
        }
    }

    fn quartiles(&self) -> Option<Quartiles> {
        let ticks = &self.decision_ticks;
        (!ticks.is_empty()).then(|| Quartiles {
            p25: ticks[ticks.len() / 4],
            median: ticks[ticks.len() / 2],
            p75: ticks[ticks.len() * 3 / 4],
        })
    }
}

fn level_record(label: &str, played: &[&FactorialMatch]) -> LevelRecord {
    let tally = Tally::of(played);
    let victories = tally.seat_wins[0] + tally.seat_wins[1];
    LevelRecord {
        level: label.to_string(),
        matches: tally.matches,
        victories,
        draws: tally.draws,
        undecided: tally.undecided,
        censored_percent: tally.censored_percent(),
        seat_wins: tally.seat_wins,
        seat0_win_rate: (victories > 0)
            .then(|| f64::from(tally.seat_wins[0]) / f64::from(victories)),
        wilson: (victories > 0).then(|| wilson(tally.seat_wins[0], victories)),
        decision_ticks: tally.quartiles(),
    }
}

fn roster_record(matches: &[FactorialMatch]) -> Option<RosterRecord> {
    let mut wins = [0u32; 2];
    for m in matches {
        let SweepOutcome::Victory { seat } = m.outcome else {
            continue;
        };
        if m.factions[0] == m.factions[1] {
            continue;
        }
        wins[usize::from(m.factions[usize::from(seat)] == roster(Faction::Cupric))] += 1;
    }
    let total = wins[0] + wins[1];
    (total > 0).then(|| RosterRecord {
        ferrous_wins: wins[0],
        cupric_wins: wins[1],
        ferrous_win_rate: f64::from(wins[0]) / f64::from(total),
        wilson: wilson(wins[0], total),
    })
}

/// The 95% Wilson score interval for `wins` of `n` — the interval that
/// stays inside 0..1 and stays honest at the small per-cell counts a
/// factorial design produces.
fn wilson(wins: u32, n: u32) -> [f64; 2] {
    const Z: f64 = 1.959_963_984_540_054;
    let n = f64::from(n);
    let p = f64::from(wins) / n;
    let denominator = 1.0 + Z * Z / n;
    let centre = (p + Z * Z / (2.0 * n)) / denominator;
    let half = Z / denominator * (p * (1.0 - p) / n + Z * Z / (4.0 * n * n)).sqrt();
    [(centre - half).max(0.0), (centre + half).min(1.0)]
}

/// Plays one cell on one seed. The all-baseline cell is a plain
/// Overseer-vs-Overseer match: the scenario as authored and seat-order
/// commands.
fn play(base: &Scenario, seed: u64, cell: Cell, max_ticks: u64) -> Result<FactorialMatch> {
    let mut sc = if cell[Factor::Geometry.index()] == 1 {
        rotate_180(base)?
    } else {
        base.clone()
    };
    sc.seed = seed;
    let factions = FACTION_CELLS[usize::from(cell[Factor::Faction.index()])];
    for (seat, faction) in factions.iter().enumerate() {
        sc.retint_seat(seat, *faction);
    }
    permute_spawn_order(&mut sc, cell[Factor::Spawn.index()]);

    let mut state: State = sc.build().context("building scenario")?;
    let mut bots: Vec<Brain> = (0u8..2)
        .map(|seat| Brain::overseer(PlayerId(seat), seed))
        .collect();
    let order: [usize; 2] = if cell[Factor::Command.index()] == 1 {
        [1, 0]
    } else {
        [0, 1]
    };
    for _ in 0..max_ticks {
        let mut commands = Vec::new();
        for &seat in &order {
            commands.extend(bots[seat].act(&state));
        }
        state.tick(&commands);
        if state.result().is_some() {
            break;
        }
    }
    let outcome = match state.result() {
        Some(GameResult::Victory { .. }) => SweepOutcome::Victory {
            seat: state
                .winners()
                .first()
                .expect("a 1v1 victory names its seat")
                .0,
        },
        Some(GameResult::Draw) => SweepOutcome::Draw,
        None => SweepOutcome::Undecided,
    };
    Ok(FactorialMatch {
        seed,
        cell: cell.to_vec(),
        levels: Vec::new(),
        factions: factions.map(|f| roster(f).to_string()),
        ticks: state.current_tick(),
        outcome,
        hash: state.hash(),
    })
}

/// Runs the design, prints the verdict, and optionally lands the raw
/// JSON for the record — the CLI entry.
pub fn factorial_report(
    scenario: &str,
    enabled: &[Factor],
    seeds: u64,
    max_ticks: u64,
    seed_base: u64,
    out: Option<&str>,
) -> Result<()> {
    let report = run_factorial(scenario, enabled, seeds, max_ticks, seed_base)?;
    println!(
        "\nFACTORIAL FAIRNESS  ·  {}  ·  Overseer both seats  ·  {} cells x {} seeds = {} matches  ·  cap {}",
        report.scenario, report.cells, report.seeds, report.matches_played, report.max_ticks
    );
    println!(
        "factors: {}\ndecided {} ({} victories, {} draws)  ·  undecided {} ({:.1}% censored)",
        report.factors.join(", "),
        report.victories + report.draws,
        report.victories,
        report.draws,
        report.undecided,
        report.censored_percent,
    );

    println!("\nper-factor marginals  ·  response: seat 0's share of victories, 95% Wilson");
    for factor in &report.per_factor {
        for (i, level) in factor.per_level.iter().enumerate() {
            let head = if i == 0 { factor.factor.as_str() } else { "" };
            let rate = match (level.seat0_win_rate, level.wilson) {
                (Some(rate), Some([lo, hi])) => format!(
                    "{:>5.1}% [{:>4.1}, {:>4.1}]",
                    100.0 * rate,
                    100.0 * lo,
                    100.0 * hi
                ),
                _ => "     -            ".to_string(),
            };
            let ticks = level.decision_ticks.map_or_else(
                || "-".to_string(),
                |q| format!("{} / {} / {}", q.p25, q.median, q.p75),
            );
            println!(
                "  {head:<12} {:<12} {:>4} matches  ·  seat0 {:>4} / seat1 {:>4}  ·  {rate}  ·  \
                 ticks {ticks}  ·  censored {:.1}%",
                level.level,
                level.matches,
                level.seat_wins[0],
                level.seat_wins[1],
                level.censored_percent,
            );
        }
    }

    if let Some(roster) = &report.roster {
        println!(
            "\nroster marginal (mixed-roster cells)  ·  ferrous {} / cupric {}  ·  \
             ferrous {:.1}% [{:.1}, {:.1}]",
            roster.ferrous_wins,
            roster.cupric_wins,
            100.0 * roster.ferrous_win_rate,
            100.0 * roster.wilson[0],
            100.0 * roster.wilson[1],
        );
    }

    println!("\ncell table  ·  interactions the marginals average away");
    let header: Vec<String> = report.factors.iter().map(|f| format!("{f:<12}")).collect();
    println!(
        "  {} {:>4} {:>6} {:>6} {:>5} {:>6} {:>8} {:>9}",
        header.join(" "),
        "n",
        "seat0",
        "seat1",
        "draw",
        "undec",
        "cens%",
        "median"
    );
    for cell in &report.per_cell {
        let levels: Vec<String> = cell.levels.iter().map(|l| format!("{l:<12}")).collect();
        println!(
            "  {} {:>4} {:>6} {:>6} {:>5} {:>6} {:>7.1}% {:>9}",
            levels.join(" "),
            cell.matches,
            cell.seat_wins[0],
            cell.seat_wins[1],
            cell.draws,
            cell.undecided,
            cell.censored_percent,
            cell.median_decision_tick
                .map_or_else(|| "-".to_string(), |t| t.to_string()),
        );
    }

    if let Some(path) = out {
        std::fs::write(path, serde_json::to_string_pretty(&report)?)?;
        println!("\nraw record: {path}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic(factions: [&str; 2], outcome: SweepOutcome, ticks: u64) -> FactorialMatch {
        FactorialMatch {
            seed: 7,
            cell: vec![0],
            levels: vec!["roster:base".into()],
            factions: [factions[0].into(), factions[1].into()],
            ticks,
            outcome,
            hash: 0,
        }
    }

    /// The audit measured every `.then(...)` in the record builders at
    /// zero execution: no test ever produced a victory, so win rates,
    /// intervals, and quartiles were unverifiable. Hand-built matches
    /// exercise the full response surface without a simulation.
    #[test]
    fn level_records_compute_rates_intervals_and_quartiles() {
        let matches = [
            synthetic(
                ["ferrous", "cupric"],
                SweepOutcome::Victory { seat: 0 },
                100,
            ),
            synthetic(
                ["ferrous", "cupric"],
                SweepOutcome::Victory { seat: 0 },
                200,
            ),
            synthetic(
                ["ferrous", "cupric"],
                SweepOutcome::Victory { seat: 0 },
                300,
            ),
            synthetic(
                ["ferrous", "cupric"],
                SweepOutcome::Victory { seat: 1 },
                400,
            ),
            synthetic(["ferrous", "cupric"], SweepOutcome::Draw, 250),
            synthetic(["ferrous", "cupric"], SweepOutcome::Undecided, 999),
        ];
        let refs: Vec<&FactorialMatch> = matches.iter().collect();
        let record = level_record("roster:base", &refs);
        assert_eq!(record.matches, 6);
        assert_eq!(record.victories, 4);
        assert_eq!(record.draws, 1);
        assert_eq!(record.undecided, 1);
        assert_eq!(record.seat_wins, [3, 1]);
        assert_eq!(record.seat0_win_rate, Some(0.75));
        let [lo, hi] = record.wilson.expect("victories imply an interval");
        assert!(lo < 0.75 && 0.75 < hi);
        assert!((0.0..=1.0).contains(&lo) && (0.0..=1.0).contains(&hi));
        let quartiles = record
            .decision_ticks
            .expect("decided matches imply quartiles");
        assert!(quartiles.p25 <= quartiles.median && quartiles.median <= quartiles.p75);

        let empty = level_record("roster:base", &[]);
        assert_eq!(empty.seat0_win_rate, None);
        assert_eq!(empty.wilson, None);
        assert!(empty.decision_ticks.is_none());
    }

    /// Roster attribution is by the WINNING seat's faction, not the
    /// seat index — a CF cell's seat-0 win is a Cupric win.
    #[test]
    fn roster_records_attribute_wins_to_factions_not_seats() {
        let matches = vec![
            synthetic(
                ["ferrous", "cupric"],
                SweepOutcome::Victory { seat: 0 },
                100,
            ),
            synthetic(
                ["ferrous", "cupric"],
                SweepOutcome::Victory { seat: 1 },
                100,
            ),
            synthetic(
                ["cupric", "ferrous"],
                SweepOutcome::Victory { seat: 0 },
                100,
            ),
            synthetic(
                ["ferrous", "ferrous"],
                SweepOutcome::Victory { seat: 0 },
                100,
            ),
            synthetic(["ferrous", "cupric"], SweepOutcome::Draw, 100),
        ];
        let record = roster_record(&matches).expect("cross-faction victories exist");
        assert_eq!(record.ferrous_wins, 1, "only the FC seat-0 win is Ferrous");
        assert_eq!(
            record.cupric_wins, 2,
            "the FC seat-1 and CF seat-0 wins are Cupric"
        );
        assert_eq!(record.ferrous_win_rate, 1.0 / 3.0);

        assert!(
            roster_record(&[synthetic(
                ["ferrous", "ferrous"],
                SweepOutcome::Victory { seat: 0 },
                100
            )])
            .is_none(),
            "mirror matches carry no roster signal"
        );
    }

    /// Rotating twice is the identity — the transform that would
    /// silently hand the seats different worlds is exactly the one a
    /// fairness verdict cannot survive.
    #[test]
    fn rotation_is_an_involution_and_preserves_the_world() {
        let base = crate::runner::load_scenario("skirmish").unwrap();
        let once = rotate_180(&base).unwrap();
        assert_ne!(once.map, base.map, "the anchors actually moved");
        assert_eq!(rotate_180(&once).unwrap(), base);

        let before = base.build().unwrap();
        let after = once.build().unwrap();
        let open = |state: &State| {
            let map = state.map();
            (0..map.height())
                .flat_map(|y| (0..map.width()).map(move |x| chassis::grid::TilePos::new(x, y)))
                .filter(|t| state.passable(*t))
                .count()
        };
        let scrap = |state: &State| {
            let map = state.map();
            (0..map.height())
                .flat_map(|y| (0..map.width()).map(move |x| chassis::grid::TilePos::new(x, y)))
                .filter_map(|t| map.tile(t))
                .map(|tile| u64::from(tile.scrap))
                .sum::<u64>()
        };
        assert_eq!(open(&before), open(&after));
        assert_eq!(scrap(&before), scrap(&after));
        assert_eq!(before.units().len(), after.units().len());
        assert_eq!(before.buildings().len(), after.buildings().len());
    }

    /// An asymmetric map is refused rather than rotated into a rigged
    /// verdict.
    #[test]
    fn an_asymmetric_map_refuses_rotation() {
        let mut base = crate::runner::load_scenario("skirmish").unwrap();
        let mut row: Vec<char> = base.map[10].chars().collect();
        let x = row
            .iter()
            .position(|c| *c == '.')
            .expect("the basin has open ground");
        row[x] = '#';
        base.map[10] = row.into_iter().collect();
        let err = rotate_180(&base).unwrap_err().to_string();
        assert!(err.contains("not 180-symmetric"), "{err}");
    }

    /// The spawn lever moves the low id range and nothing else.
    #[test]
    fn the_spawn_lever_hands_the_low_ids_to_the_named_seat() {
        let base = crate::runner::load_scenario("skirmish").unwrap();
        for first in [0u8, 1] {
            let mut sc = base.clone();
            permute_spawn_order(&mut sc, first);
            assert_eq!(sc.units.len(), base.units.len());
            let state = sc.build().unwrap();
            let low = state
                .units()
                .iter()
                .min_by_key(|u| u.id.0)
                .expect("the basin starts units");
            assert_eq!(low.player, PlayerId(first));
        }
    }

    /// The all-baseline cell must reproduce, bit for bit, a reference
    /// run that seats [`Brain::overseer`] per seat and steps the sim
    /// directly — proof that the harness transforms are neutral when
    /// every lever sits at its baseline.
    #[test]
    fn the_baseline_cell_reproduces_a_direct_overseer_run() {
        let base = crate::runner::load_scenario("skirmish").unwrap();
        for seed in [0u64, 17, 4_242] {
            let mut sc = base.clone();
            sc.seed = seed;
            let mut state = sc.build().unwrap();
            let mut bots: Vec<Brain> = (0u8..2)
                .map(|seat| Brain::overseer(PlayerId(seat), seed))
                .collect();
            for _ in 0..200 {
                let mut commands = Vec::new();
                for bot in &mut bots {
                    commands.extend(bot.act(&state));
                }
                state.tick(&commands);
                if state.result().is_some() {
                    break;
                }
            }
            let probed = play(&base, seed, [0; FACTOR_COUNT], 200).unwrap();
            assert_eq!(probed.hash, state.hash(), "seed {seed}");
            assert_eq!(probed.ticks, state.current_tick(), "seed {seed}");
        }
    }

    /// Every cell is played on every seed, every match lands in exactly
    /// one cell, and the marginals account for all of them.
    #[test]
    fn the_design_accounts_for_every_cell() {
        let report = run_factorial("skirmish", &Factor::ALL, 1, 30, 7_000).unwrap();
        assert_eq!(report.cells, 4 * 2 * 2 * 2);
        assert_eq!(report.matches_played as usize, report.cells);
        assert_eq!(report.per_cell.len(), report.cells);
        assert!(report.per_cell.iter().all(|c| c.matches == 1));
        for factor in &report.per_factor {
            let counted: u32 = factor.per_level.iter().map(|l| l.matches).sum();
            assert_eq!(counted, report.matches_played, "{}", factor.factor);
        }
        // The faction levels are absolute rosters, not relative flips.
        let ff = report
            .matches
            .iter()
            .find(|m| m.cell[Factor::Faction.index()] == 2)
            .unwrap();
        assert_eq!(ff.factions, ["ferrous".to_string(), "ferrous".to_string()]);
    }

    /// A subset design pins the factors it left out at their baseline.
    #[test]
    fn a_subset_design_pins_the_disabled_factors() {
        let report = run_factorial(
            "skirmish",
            &[Factor::Command, Factor::Geometry],
            1,
            20,
            7_000,
        )
        .unwrap();
        assert_eq!(report.cells, 4);
        assert_eq!(report.factors, ["command", "geometry"]);
        for m in &report.matches {
            assert_eq!(m.cell[Factor::Faction.index()], 0);
            assert_eq!(m.cell[Factor::Spawn.index()], 0);
            assert_eq!(m.levels.len(), 2);
        }
    }

    /// Columns and rows are both read off `Factor::ALL`, so a
    /// caller-ordered list is normalized rather than labelled apart —
    /// and a repeated factor is refused instead of silently doubling
    /// the design.
    #[test]
    fn the_design_normalizes_its_factor_order_and_refuses_repeats() {
        let report = run_factorial(
            "skirmish",
            &[Factor::Geometry, Factor::Faction],
            1,
            20,
            7_000,
        )
        .unwrap();
        assert_eq!(report.factors, ["faction", "geometry"]);
        assert_eq!(
            report.per_factor[0].factor, "faction",
            "the marginals follow the same order as the columns"
        );
        let err = run_factorial(
            "skirmish",
            &[Factor::Geometry, Factor::Geometry],
            1,
            20,
            7_000,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("only once"), "{err}");
    }

    /// The interval brackets the point estimate and never leaves 0..1.
    #[test]
    fn the_wilson_interval_stays_inside_the_unit_range() {
        for (wins, n) in [(0u32, 1u32), (1, 1), (0, 8), (8, 8), (3, 7), (40, 95)] {
            let [lo, hi] = wilson(wins, n);
            let p = f64::from(wins) / f64::from(n);
            assert!((0.0..=1.0).contains(&lo) && (0.0..=1.0).contains(&hi));
            assert!(lo <= p && p <= hi, "{wins}/{n} -> [{lo}, {hi}]");
        }
    }

    /// An unknown factor names the ones that exist.
    #[test]
    fn an_unknown_factor_lists_the_known_ones() {
        let err = Factor::parse("weather").unwrap_err().to_string();
        assert!(err.contains("faction") && err.contains("geometry"), "{err}");
    }
}
