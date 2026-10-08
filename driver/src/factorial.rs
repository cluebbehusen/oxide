//! Factorial measurements of a configured bot interacting with the simulation.
//! Faction, map geometry, starting unit-id order, and command order are crossed
//! on the same simulation seeds while every seat uses the same complete profile.
//!
//! Reports include seat-zero win rate over decided matches, Wilson intervals,
//! decision-tick quartiles, and the censored share. The full cell table exposes
//! interactions that aggregate marginals can hide. These outcomes depend on
//! the controller's response to each configuration as well as the game rules.
//!
//! Nothing in the probe changes the sim: every cell is a transform of
//! the scenario or of how the harness assembles the tick, and the
//! all-baseline cell reproduces a direct configured-bot mirror run bit
//! for bit (a test pins that against the sim stepped by hand).

use crate::sweep::{SweepOutcome, Tally, outcome_of, play_mirror, wilson};
use anyhow::{Context, Result};
use chassis::grid::as_index;
#[cfg(test)]
use oxide_sim::PlayerId;
use oxide_sim::scenario::Scenario;
use oxide_sim::{BuildingKind, Faction};
use serde::Serialize;

/// How many levers the design carries.
pub const FACTOR_COUNT: usize = 4;

/// One lever of the design. Every factor is something the shipped game
/// ties to the seat number. Marginals measure its effect on the configured
/// controller matchup, including the controller's response to that change.
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
    /// Exact shared controller profile for this measurement.
    #[serde(serialize_with = "crate::sweep::serialize_bot_config")]
    pub bot_config: oxide_sim::scenario::BotConfig,
    /// Simulation rules used by this measurement.
    pub sim_version: String,
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
/// Refuses a map whose terrain is not exactly 180-symmetric with Foundry and
/// Extractor footprint markers read as ground. Rotating an asymmetric map would
/// hand the seats different worlds, making every later verdict about terrain
/// rather than the seat.
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
    let is_frame = |c: char| c == 'E';
    let ground = |c: char| {
        if is_anchor(c) || is_frame(c) { '.' } else { c }
    };
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
    let (fw, fh) = (as_index(fw), as_index(fh));
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
    // `E` likewise names the top-left of a 2x2 Extractor frame. Rotating
    // the marker as a point would shift the gameplay footprint by one tile.
    let (ew, eh) = BuildingKind::Extractor.base_stats().size;
    let (ew, eh) = (as_index(ew), as_index(eh));
    for (y, row) in rows.iter().enumerate() {
        for (x, &c) in row.iter().enumerate() {
            if !is_frame(c) {
                continue;
            }
            let (Some(ty), Some(tx)) = (
                height.checked_sub(eh).and_then(|h| h.checked_sub(y)),
                width.checked_sub(ew).and_then(|w| w.checked_sub(x)),
            ) else {
                anyhow::bail!(
                    "{}'s Extractor frame at ({x}, {y}) has no room for its footprint",
                    base.name
                );
            };
            map[ty][tx] = c;
        }
    }

    let mut out = base.clone();
    out.map = map.into_iter().map(|r| r.into_iter().collect()).collect();
    let (w, h) = (
        i32::try_from(width).expect("map extents fit in i32"),
        i32::try_from(height).expect("map extents fit in i32"),
    );
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
            for level in 0..u8::try_from(factor.levels().len()).expect("lengths fit in u8") {
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
fn labels(enabled: &[Factor], cell: Cell) -> Vec<String> {
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
    config: oxide_sim::scenario::BotConfig,
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
        "the factorial probe requires a 1v1 matchup; {} has {} seats",
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
        let played = play(&base, seed, cell, max_ticks, config)?;
        eprintln!(
            "  {} · seed {} · {} ticks · {:?}",
            labels(enabled, cell).join(" "),
            played.seed,
            played.ticks,
            played.outcome
        );
        Ok(FactorialMatch {
            levels: labels(enabled, cell),
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
            let tally = tally(&played);
            CellRecord {
                levels: labels(enabled, *cell),
                matches: tally.matches,
                seat_wins: tally.seat_wins,
                draws: tally.draws,
                undecided: tally.undecided,
                censored_percent: tally.censored_percent(),
                median_decision_tick: tally.quantile(1, 2),
            }
        })
        .collect();

    let overall = tally(&matches.iter().collect::<Vec<_>>());
    Ok(FactorialReport {
        bot_config: config,
        sim_version: oxide_sim::SIM_VERSION.to_string(),
        scenario: base.name,
        seeds,
        seed_base,
        max_ticks,
        factors: enabled.iter().map(|f| f.key().to_string()).collect(),
        cells: cells.len(),
        matches_played: overall.matches,
        victories: overall.victories(),
        draws: overall.draws,
        undecided: overall.undecided,
        censored_percent: overall.censored_percent(),
        per_factor,
        roster: roster_record(&matches),
        per_cell,
        matches,
    })
}

fn tally(played: &[&FactorialMatch]) -> Tally {
    Tally::of(played.iter().map(|m| (m.outcome, m.ticks)))
}

fn quartiles(tally: &Tally) -> Option<Quartiles> {
    Some(Quartiles {
        p25: tally.quantile(1, 4)?,
        median: tally.quantile(1, 2)?,
        p75: tally.quantile(3, 4)?,
    })
}

fn level_record(label: &str, played: &[&FactorialMatch]) -> LevelRecord {
    let tally = tally(played);
    let victories = tally.victories();
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
        decision_ticks: quartiles(&tally),
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

/// Plays one cell on one seed. The all-baseline cell is a plain
/// configured-bot mirror match: the scenario as authored and seat-order
/// commands.
fn play(
    base: &Scenario,
    seed: u64,
    cell: Cell,
    max_ticks: u64,
    config: oxide_sim::scenario::BotConfig,
) -> Result<FactorialMatch> {
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

    let command_order = if cell[Factor::Command.index()] == 1 {
        [1, 0]
    } else {
        [0, 1]
    };
    let state = play_mirror(sc, config, max_ticks, command_order)?;
    Ok(FactorialMatch {
        seed,
        cell: cell.to_vec(),
        levels: Vec::new(),
        factions: factions.map(|f| roster(f).to_string()),
        ticks: state.current_tick(),
        outcome: outcome_of(&state),
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
    config: oxide_sim::scenario::BotConfig,
) -> Result<()> {
    let report = run_factorial(scenario, enabled, seeds, max_ticks, seed_base, config)?;
    println!("controller: {config:?}; sim {}", oxide_sim::SIM_VERSION);
    println!(
        "\nFACTORIAL MATCHUPS  ·  {}  ·  current controller both seats  ·  {} cells x {} seeds = {} matches  ·  cap {}",
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
mod tests;
