//! The decisiveness sweep: N seeds of bot-vs-bot on one 1v1 scenario at
//! one ladder level, each seed played twice — once with personalities
//! dealt by the sim, once with the same two values exchanged between
//! the seats. Where `balance-probe` reads what armies were made of,
//! this reads whether games *end*: decided/undecided counts, seat bias
//! that survives the personality exchange, and decision-tick medians.
//! The 0.12 bot phases gate on it.
//!
//! The exchange swaps only the personality knob; each seat keeps its
//! own blunder stream, which is a per-seat variable at any degraded
//! level. A lean that shows up in both orientations is the map or the
//! engine, not personality luck.
//!
//! `duel` is the sibling instrument: two named profiles (a ladder rung,
//! optionally with candidate skill/cadence dial overrides) fight across
//! N seeds with the sides exchanging seats — the head-to-head the
//! Medium re-metering selects candidates on, since the in-tree ladder
//! test is a tripwire, not a measuring stick.

use anyhow::{Context, Result};
use oxide_sim::bot::{Level, NeuralBot, QuantNet, deal_aggression, seat_bots};
use oxide_sim::scenario::{BotConfig, Scenario};
use oxide_sim::{GameResult, PlayerId, State};
use serde::Serialize;

/// How one sweep match ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum SweepOutcome {
    /// A team won; `seat` is its sole (1v1) seat.
    Victory {
        /// The winning seat.
        seat: u8,
    },
    /// Mutual Foundry death on one tick.
    Draw,
    /// The tick cap arrived first.
    Undecided,
}

/// One match of the sweep.
#[derive(Debug, Clone, Serialize)]
pub struct SweepMatch {
    /// Scenario seed this match ran under.
    pub seed: u64,
    /// Whether the two seats' dealt personalities were exchanged.
    pub swapped: bool,
    /// The personality each seat actually played, seat-indexed.
    pub aggression: [u32; 2],
    /// Final tick: the decision tick, or the cap.
    pub ticks: u64,
    /// How it ended.
    pub outcome: SweepOutcome,
}

/// The sweep's aggregate verdict.
#[derive(Debug, Clone, Serialize)]
pub struct SweepReport {
    /// Scenario name.
    pub scenario: String,
    /// Ladder level swept.
    pub level: String,
    /// Seeds swept (each played in both orientations).
    pub seeds: u64,
    /// Tick cap per match.
    pub max_ticks: u64,
    /// Matches ending in a victory.
    pub victories: u32,
    /// Mutual-death draws.
    pub draws: u32,
    /// Matches that hit the cap.
    pub undecided: u32,
    /// Victories by seat over all matches.
    pub seat_wins: [u32; 2],
    /// Undecided counts by orientation: [dealt, swapped].
    pub undecided_by_orientation: [u32; 2],
    /// Median decision tick over decided matches.
    pub median_decision_tick: Option<u64>,
    /// Mean personality of winning seats over victories.
    pub mean_winner_aggression: Option<f64>,
    /// Mean personality of losing seats over victories.
    pub mean_loser_aggression: Option<f64>,
    /// Every match, in (seed, orientation) order.
    pub matches: Vec<SweepMatch>,
}

/// Runs the sweep headless and returns the aggregate. Matches fan out
/// across a worker pool pulling from a shared queue, like the map
/// sweeps — every match is an independent deterministic sim.
pub fn run_sweep(
    scenario: &str,
    level: Level,
    seeds: u64,
    max_ticks: u64,
    seed_base: u64,
) -> Result<SweepReport> {
    let base = crate::runner::load_scenario(scenario)?;
    anyhow::ensure!(
        base.players.len() == 2,
        "the sweep reads 1v1 decisiveness; {} has {} seats",
        base.name,
        base.players.len()
    );

    let jobs: Vec<(u64, bool)> = (0..seeds)
        .flat_map(|offset| [(offset, false), (offset, true)])
        .collect();
    // The pool returns results in job order, so the record is ordered
    // by (seed, orientation) without a second sort.
    let matches = crate::pool::fan_out(&jobs, |&(offset, swapped)| {
        let m = play(&base, level, seed_base + offset, swapped, max_ticks)?;
        eprintln!(
            "  seed {} {} · {} ticks · {:?}",
            m.seed,
            if m.swapped { "swap" } else { "deal" },
            m.ticks,
            m.outcome
        );
        Ok(m)
    })?;

    let mut victories = 0u32;
    let mut draws = 0u32;
    let mut undecided = 0u32;
    let mut seat_wins = [0u32; 2];
    let mut undecided_by_orientation = [0u32; 2];
    let mut decision_ticks: Vec<u64> = Vec::new();
    let mut winner_aggression: Vec<u32> = Vec::new();
    let mut loser_aggression: Vec<u32> = Vec::new();
    for m in &matches {
        match m.outcome {
            SweepOutcome::Victory { seat } => {
                victories += 1;
                seat_wins[seat as usize] += 1;
                decision_ticks.push(m.ticks);
                winner_aggression.push(m.aggression[seat as usize]);
                loser_aggression.push(m.aggression[1 - seat as usize]);
            }
            SweepOutcome::Draw => {
                draws += 1;
                decision_ticks.push(m.ticks);
            }
            SweepOutcome::Undecided => {
                undecided += 1;
                undecided_by_orientation[usize::from(m.swapped)] += 1;
            }
        }
    }
    decision_ticks.sort_unstable();
    let mean = |values: &[u32]| {
        (!values.is_empty())
            .then(|| values.iter().map(|&a| f64::from(a)).sum::<f64>() / values.len() as f64)
    };
    Ok(SweepReport {
        scenario: base.name.clone(),
        level: format!("{level:?}"),
        seeds,
        max_ticks,
        victories,
        draws,
        undecided,
        seat_wins,
        undecided_by_orientation,
        median_decision_tick: (!decision_ticks.is_empty())
            .then(|| decision_ticks[decision_ticks.len() / 2]),
        mean_winner_aggression: mean(&winner_aggression),
        mean_loser_aggression: mean(&loser_aggression),
        matches,
    })
}

/// Runs the sweep, prints the verdict, and optionally lands the raw
/// JSON for the record — the CLI entry.
pub fn sweep_report(
    scenario: &str,
    level: Level,
    seeds: u64,
    max_ticks: u64,
    seed_base: u64,
    out: Option<&str>,
) -> Result<()> {
    let report = run_sweep(scenario, level, seeds, max_ticks, seed_base)?;
    println!(
        "\nSEED SWEEP  ·  {}  ·  level {}  ·  {} seeds x 2 orientations  ·  cap {}",
        report.scenario, report.level, report.seeds, report.max_ticks
    );
    println!(
        "decided {} ({} victories, {} draws)  ·  undecided {} (dealt {}, swapped {})",
        report.victories + report.draws,
        report.victories,
        report.draws,
        report.undecided,
        report.undecided_by_orientation[0],
        report.undecided_by_orientation[1],
    );
    println!(
        "seat wins: seat0 {}, seat1 {}",
        report.seat_wins[0], report.seat_wins[1]
    );
    if let Some(median) = report.median_decision_tick {
        println!("median decision tick: {median}");
    }
    if let (Some(winner), Some(loser)) =
        (report.mean_winner_aggression, report.mean_loser_aggression)
    {
        println!("mean aggression: winners {winner:.0}, losers {loser:.0}");
    }
    if let Some(path) = out {
        std::fs::write(path, serde_json::to_string_pretty(&report)?)?;
        println!("raw record: {path}");
    }
    Ok(())
}

/// Plays one match: build, think, step, stop at the decision or the cap.
fn play(
    base: &Scenario,
    level: Level,
    seed: u64,
    swapped: bool,
    max_ticks: u64,
) -> Result<SweepMatch> {
    // The shipped dealing itself — one definition, no replicated
    // stream: same seed, same personalities as the game deals.
    let dealt = [0u8, 1u8].map(|seat| deal_aggression(seed, PlayerId(seat)));
    let mut sc = base.clone();
    sc.seed = seed;
    for (i, player) in sc.players.iter_mut().enumerate() {
        player.bot = true;
        // The dealt leg passes None so the shipped dealing path runs
        // end-to-end; the swapped leg passes the same two values
        // exchanged.
        let aggression = swapped.then(|| dealt[1 - i]);
        player.bot_config = Some(BotConfig { level, aggression });
    }
    let mut state: State = sc.build().context("building scenario")?;
    let mut bots = seat_bots(&sc);
    for _ in 0..max_ticks {
        crate::runner::step(&mut state, &mut bots, None);
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
    Ok(SweepMatch {
        seed,
        swapped,
        aggression: if swapped { [dealt[1], dealt[0]] } else { dealt },
        ticks: state.current_tick(),
        outcome,
    })
}

/// One side of a duel: a ladder rung, optionally with candidate
/// skill/cadence overrides — the re-metering experiments probe points
/// between the shipped rungs. Personality stays the shipped per-seat
/// deal; blunder derives from skill, as shipped.
#[derive(Debug, Clone, Serialize)]
pub struct DuelSide {
    /// The base rung.
    pub level: Level,
    /// Skill-knob override (None: the rung's own).
    pub skill: Option<u32>,
    /// Cadence override (None: the rung's own).
    pub cadence: Option<u64>,
}

impl DuelSide {
    /// The profile's display name, dials included.
    pub fn label(&self) -> String {
        let mut label = format!("{:?}", self.level).to_lowercase();
        if let Some(skill) = self.skill {
            label.push_str(&format!("/s{skill}"));
        }
        if let Some(cadence) = self.cadence {
            label.push_str(&format!("/c{cadence}"));
        }
        label
    }

    fn bot(&self, player: PlayerId, seed: u64, faction: oxide_sim::Faction) -> NeuralBot {
        self.bot_with_aggression(player, seed, faction, deal_aggression(seed, player))
    }

    fn bot_with_aggression(
        &self,
        player: PlayerId,
        seed: u64,
        faction: oxide_sim::Faction,
        aggression: u32,
    ) -> NeuralBot {
        NeuralBot::with_profile(
            player,
            self.cadence.unwrap_or_else(|| self.level.cadence()),
            QuantNet::ladder().clone(),
            self.skill.unwrap_or_else(|| self.level.skill()),
            aggression,
            faction,
            0,
            seed,
        )
    }
}

/// The duel's aggregate verdict.
#[derive(Debug, Clone, Serialize)]
pub struct DuelReport {
    /// Scenario name.
    pub scenario: String,
    /// Side A's profile label.
    pub a: String,
    /// Side B's profile label.
    pub b: String,
    /// Seeds fought (each from both seats).
    pub seeds: u64,
    /// Tick cap per match.
    pub max_ticks: u64,
    /// Side A's victories.
    pub a_wins: u32,
    /// Side B's victories.
    pub b_wins: u32,
    /// Mutual-death draws.
    pub draws: u32,
    /// Matches that hit the cap.
    pub undecided: u32,
    /// Side A's victories split by the seat it held: [seat0, seat1].
    pub a_wins_by_seat: [u32; 2],
    /// Median decision tick over decided matches.
    pub median_decision_tick: Option<u64>,
}

/// One finished duel match: side A's seat, the final tick, the
/// winning seat, and whether the decision was a mutual-death draw.
type DuelEntry = (usize, u64, Option<u8>, bool);

/// Fights `a` against `b` across `seeds`, each seed from both seats.
/// Sides carry their profiles with them when they change chairs;
/// personalities stay dealt per seat, so the exchange isolates the
/// profile difference from both seat bias and personality luck.
pub fn run_duel(
    scenario: &str,
    a: &DuelSide,
    b: &DuelSide,
    seeds: u64,
    max_ticks: u64,
    seed_base: u64,
) -> Result<DuelReport> {
    let base = crate::runner::load_scenario(scenario)?;
    anyhow::ensure!(
        base.players.len() == 2,
        "the duel reads 1v1 head-to-heads; {} has {} seats",
        base.name,
        base.players.len()
    );

    // (offset, the seat side A holds)
    let jobs: Vec<(u64, usize)> = (0..seeds)
        .flat_map(|offset| [(offset, 0), (offset, 1)])
        .collect();
    // (a_seat, decision tick, winning seat, drawn)
    let entries: Vec<DuelEntry> = crate::pool::fan_out(&jobs, |&(offset, a_seat)| {
        play_duel(&base, a, b, seed_base + offset, a_seat, max_ticks)
    })?;

    let mut report = DuelReport {
        scenario: base.name.clone(),
        a: a.label(),
        b: b.label(),
        seeds,
        max_ticks,
        a_wins: 0,
        b_wins: 0,
        draws: 0,
        undecided: 0,
        a_wins_by_seat: [0; 2],
        median_decision_tick: None,
    };
    let mut decision_ticks: Vec<u64> = Vec::new();
    for (a_seat, ticks, winner, drawn) in entries {
        match winner {
            Some(seat) => {
                decision_ticks.push(ticks);
                if usize::from(seat) == a_seat {
                    report.a_wins += 1;
                    report.a_wins_by_seat[a_seat] += 1;
                } else {
                    report.b_wins += 1;
                }
            }
            None if drawn => {
                decision_ticks.push(ticks);
                report.draws += 1;
            }
            None => report.undecided += 1,
        }
    }
    decision_ticks.sort_unstable();
    report.median_decision_tick =
        (!decision_ticks.is_empty()).then(|| decision_ticks[decision_ticks.len() / 2]);
    Ok(report)
}

/// Runs the duel and prints the verdict — the CLI entry.
pub fn duel_report(
    scenario: &str,
    a: &DuelSide,
    b: &DuelSide,
    seeds: u64,
    max_ticks: u64,
    seed_base: u64,
    out: Option<&str>,
) -> Result<()> {
    let report = run_duel(scenario, a, b, seeds, max_ticks, seed_base)?;
    println!(
        "\nDUEL  ·  {}  ·  {} vs {}  ·  {} seeds x 2 seats  ·  cap {}",
        report.scenario, report.a, report.b, report.seeds, report.max_ticks
    );
    println!(
        "{} {} - {} {}  ·  draws {}  ·  undecided {}  ·  A by seat [{}, {}]",
        report.a,
        report.a_wins,
        report.b_wins,
        report.b,
        report.draws,
        report.undecided,
        report.a_wins_by_seat[0],
        report.a_wins_by_seat[1],
    );
    if let Some(median) = report.median_decision_tick {
        println!("median decision tick: {median}");
    }
    if let Some(path) = out {
        std::fs::write(path, serde_json::to_string_pretty(&report)?)?;
        println!("raw record: {path}");
    }
    Ok(())
}

/// Plays one duel match with side A in `a_seat`; returns
/// (a_seat, final tick, winning seat, drawn).
fn play_duel(
    base: &Scenario,
    a: &DuelSide,
    b: &DuelSide,
    seed: u64,
    a_seat: usize,
    max_ticks: u64,
) -> Result<DuelEntry> {
    let mut sc = base.clone();
    sc.seed = seed;
    let mut state: State = sc.build().context("building scenario")?;
    let mut bots: Vec<NeuralBot> = sc
        .players
        .iter()
        .enumerate()
        .map(|(seat, player)| {
            let side = if seat == a_seat { a } else { b };
            side.bot(PlayerId(seat as u8), seed, player.faction)
        })
        .collect();
    for _ in 0..max_ticks {
        let mut commands = Vec::new();
        for bot in bots.iter_mut() {
            commands.extend(bot.act(&state));
        }
        state.tick(&commands);
        if state.result().is_some() {
            break;
        }
    }
    let (winner, drawn) = match state.result() {
        Some(GameResult::Victory { .. }) => (
            Some(
                state
                    .winners()
                    .first()
                    .expect("a 1v1 victory names its seat")
                    .0,
            ),
            false,
        ),
        Some(GameResult::Draw) => (None, true),
        None => (None, false),
    };
    eprintln!(
        "  seed {seed} A@{a_seat} · {} ticks · {:?}",
        state.current_tick(),
        winner
    );
    Ok((a_seat, state.current_tick(), winner, drawn))
}

/// Per-tier record on the scripted yardstick.
#[derive(Debug, Clone, Serialize)]
pub struct TierRecord {
    /// The scripted opponent tier.
    pub tier: String,
    /// Profile wins against it.
    pub wins: u32,
    /// Matches fought.
    pub matches: u32,
    /// Matches that drew or hit the cap.
    pub unresolved: u32,
    /// Median tick of the profile's victories against this tier.
    /// Pace, not count: two rungs with the same record separate here.
    pub median_victory_tick: Option<u64>,
    /// Third-quartile tick of those same victories — the grind tail a
    /// median hides.
    pub p75_victory_tick: Option<u64>,
}

/// One finished yardstick match, before the fold.
struct YardstickEntry {
    map: usize,
    tier: usize,
    won: bool,
    resolved: bool,
    ticks: u64,
}

/// Nearest-rank quantile over an already-sorted series.
fn quantile(sorted: &[u64], num: usize, den: usize) -> Option<u64> {
    (!sorted.is_empty()).then(|| sorted[(sorted.len() * num / den).min(sorted.len() - 1)])
}

/// The widened scripted-yardstick verdict for one profile — the same
/// methodology as the in-tree ladder gate (fixed 500 personality, the
/// profile vs every scripted tier from both seats), over as many seeds
/// as the recalibration wants instead of the gate's pinned 24.
#[derive(Debug, Clone, Serialize)]
pub struct YardstickReport {
    /// The measured profile's label.
    pub profile: String,
    /// Scenario name.
    pub scenario: String,
    /// Seeds per tier (each fought from both seats).
    pub seeds_per_tier: u64,
    /// Tick cap per match.
    pub max_ticks: u64,
    /// Per-tier records, gentlest first.
    pub per_tier: Vec<TierRecord>,
    /// Total wins.
    pub wins: u32,
    /// Total matches.
    pub matches: u32,
    /// Total matches that drew or hit the cap.
    pub unresolved: u32,
}

/// The yardstick across a whole scenario directory: the same profile
/// measured on every 1v1 map, per map and pooled. The ladder is
/// calibrated on one map, and duration distributions vary by an order
/// of magnitude across the roster.
#[derive(Debug, Clone, Serialize)]
pub struct YardstickSlate {
    /// The measured profile's label.
    pub profile: String,
    /// The scenario directory swept.
    pub dir: String,
    /// Seeds per tier per map (each fought from both seats).
    pub seeds_per_tier: u64,
    /// Tick cap per match.
    pub max_ticks: u64,
    /// Per-map reports, in path order.
    pub per_map: Vec<YardstickReport>,
    /// Per-tier records pooled over every map — folded from the raw
    /// matches, never from the per-map quantiles.
    pub per_tier: Vec<TierRecord>,
    /// Total wins over the slate.
    pub wins: u32,
    /// Total matches over the slate.
    pub matches: u32,
    /// Total matches that drew or hit the cap.
    pub unresolved: u32,
}

const TIERS: [oxide_sim::bot::Difficulty; 4] = {
    use oxide_sim::bot::Difficulty;
    [
        Difficulty::Scrapheap,
        Difficulty::Standard,
        Difficulty::Veteran,
        Difficulty::Prime,
    ]
};

/// Folds the entries a filter selects into one record per tier.
fn tier_records<'a>(entries: impl Iterator<Item = &'a YardstickEntry>) -> Vec<TierRecord> {
    let mut wins = [0u32; TIERS.len()];
    let mut matches = [0u32; TIERS.len()];
    let mut unresolved = [0u32; TIERS.len()];
    let mut victory_ticks: [Vec<u64>; TIERS.len()] = Default::default();
    for e in entries {
        matches[e.tier] += 1;
        unresolved[e.tier] += u32::from(!e.resolved);
        if e.won {
            wins[e.tier] += 1;
            victory_ticks[e.tier].push(e.ticks);
        }
    }
    TIERS
        .iter()
        .enumerate()
        .map(|(t, tier)| {
            victory_ticks[t].sort_unstable();
            TierRecord {
                tier: format!("{tier:?}"),
                wins: wins[t],
                matches: matches[t],
                unresolved: unresolved[t],
                median_victory_tick: quantile(&victory_ticks[t], 1, 2),
                p75_victory_tick: quantile(&victory_ticks[t], 3, 4),
            }
        })
        .collect()
}

/// Assembles one map's report from the entries fought on it.
fn map_report<'a>(
    name: &str,
    side: &DuelSide,
    seeds_per_tier: u64,
    max_ticks: u64,
    entries: impl Iterator<Item = &'a YardstickEntry>,
) -> YardstickReport {
    let per_tier = tier_records(entries);
    YardstickReport {
        profile: side.label(),
        scenario: name.to_string(),
        seeds_per_tier,
        max_ticks,
        wins: per_tier.iter().map(|t| t.wins).sum(),
        matches: per_tier.iter().map(|t| t.matches).sum(),
        unresolved: per_tier.iter().map(|t| t.unresolved).sum(),
        per_tier,
    }
}

/// Fights `side` against every scripted tier on every one of `bases`,
/// on one shared worker pool — nesting a pool per map would let the
/// thread count decide how long the slate takes.
fn yardstick_entries(
    bases: &[Scenario],
    side: &DuelSide,
    seeds_per_tier: u64,
    max_ticks: u64,
    seed_base: u64,
) -> Result<Vec<YardstickEntry>> {
    use oxide_sim::bot::Brain;
    // (map index, tier index, seed offset, the profile's seat)
    let jobs: Vec<(usize, usize, u64, u8)> = (0..bases.len())
        .flat_map(|m| {
            (0..TIERS.len()).flat_map(move |t| {
                (0..seeds_per_tier).flat_map(move |o| [(m, t, o, 0u8), (m, t, o, 1u8)])
            })
        })
        .collect();
    crate::pool::fan_out(&jobs, |&(m, t, offset, seat)| {
        let seed = seed_base + offset;
        let mut sc = bases[m].clone();
        sc.seed = seed;
        let mut state: State = sc.build().context("building scenario")?;
        let faction = sc.players[usize::from(seat)].faction;
        let mut bot = side.bot_with_aggression(PlayerId(seat), seed, faction, 500);
        let mut opp = Brain::for_tier(PlayerId(1 - seat), seed, TIERS[t]);
        for _ in 0..max_ticks {
            let mut commands = bot.act(&state);
            commands.extend(opp.act(&state));
            state.tick(&commands);
            if state.result().is_some() {
                break;
            }
        }
        let resolved = matches!(state.result(), Some(GameResult::Victory { .. }));
        let won = resolved && state.winners().contains(&PlayerId(seat));
        Ok(YardstickEntry {
            map: m,
            tier: t,
            won,
            resolved,
            ticks: state.current_tick(),
        })
    })
}

/// Measures `side` against the four scripted tiers. The profile plays
/// the gate's fixed 500 personality so the skill dials are the only
/// variable; scripted opponents are the fixed aggression yardstick the
/// cadence calibration is doctrinally measured on (head-to-head
/// neural mirrors reward patience instead).
pub fn run_yardstick(
    scenario: &str,
    side: &DuelSide,
    seeds_per_tier: u64,
    max_ticks: u64,
    seed_base: u64,
) -> Result<YardstickReport> {
    let base = crate::runner::load_scenario(scenario)?;
    anyhow::ensure!(
        base.players.len() == 2,
        "the yardstick reads 1v1; {} has {} seats",
        base.name,
        base.players.len()
    );
    let bases = [base];
    let entries = yardstick_entries(&bases, side, seeds_per_tier, max_ticks, seed_base)?;
    Ok(map_report(
        &bases[0].name,
        side,
        seeds_per_tier,
        max_ticks,
        entries.iter(),
    ))
}

/// Measures `side` on every 1v1 scenario in `dir`. Maps with any other
/// seat count are skipped, not refused — the shipped directory mixes
/// formats and the yardstick's opponent is a single scripted seat.
pub fn run_yardstick_slate(
    dir: &str,
    side: &DuelSide,
    seeds_per_tier: u64,
    max_ticks: u64,
    seed_base: u64,
) -> Result<YardstickSlate> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {dir}"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    paths.sort();
    anyhow::ensure!(!paths.is_empty(), "no scenarios under {dir}");
    let mut bases: Vec<Scenario> = Vec::new();
    for path in &paths {
        let sc = crate::runner::load_scenario(&path.to_string_lossy())?;
        if sc.players.len() == 2 {
            bases.push(sc);
        } else {
            eprintln!("  skipping {} ({} seats)", sc.name, sc.players.len());
        }
    }
    anyhow::ensure!(!bases.is_empty(), "no 1v1 scenarios under {dir}");

    let entries = yardstick_entries(&bases, side, seeds_per_tier, max_ticks, seed_base)?;
    let per_map: Vec<YardstickReport> = bases
        .iter()
        .enumerate()
        .map(|(m, base)| {
            map_report(
                &base.name,
                side,
                seeds_per_tier,
                max_ticks,
                entries.iter().filter(|e| e.map == m),
            )
        })
        .collect();
    let per_tier = tier_records(entries.iter());
    Ok(YardstickSlate {
        profile: side.label(),
        dir: dir.to_string(),
        seeds_per_tier,
        max_ticks,
        wins: per_tier.iter().map(|t| t.wins).sum(),
        matches: per_tier.iter().map(|t| t.matches).sum(),
        unresolved: per_tier.iter().map(|t| t.unresolved).sum(),
        per_map,
        per_tier,
    })
}

/// Prints one tier table, indented under whatever names it.
fn print_tiers(per_tier: &[TierRecord]) {
    for tier in per_tier {
        let pace = match (tier.median_victory_tick, tier.p75_victory_tick) {
            (Some(median), Some(p75)) => format!("win ticks median {median}, p75 {p75}"),
            _ => "no victories".to_string(),
        };
        println!(
            "  vs {:<10} {:>2}/{:<2} ({} unresolved)  ·  {pace}",
            tier.tier, tier.wins, tier.matches, tier.unresolved
        );
    }
}

/// Runs the yardstick and prints the verdict — the CLI entry.
pub fn yardstick_report(
    scenario: &str,
    side: &DuelSide,
    seeds_per_tier: u64,
    max_ticks: u64,
    seed_base: u64,
    out: Option<&str>,
) -> Result<()> {
    let report = run_yardstick(scenario, side, seeds_per_tier, max_ticks, seed_base)?;
    println!(
        "\nYARDSTICK  ·  {}  ·  {}  ·  {} seeds/tier x 2 seats  ·  cap {}",
        report.scenario, report.profile, report.seeds_per_tier, report.max_ticks
    );
    print_tiers(&report.per_tier);
    println!(
        "total {}/{}  ·  {} unresolved",
        report.wins, report.matches, report.unresolved
    );
    if let Some(path) = out {
        std::fs::write(path, serde_json::to_string_pretty(&report)?)?;
        println!("raw record: {path}");
    }
    Ok(())
}

/// Runs the yardstick across a scenario directory and prints the
/// verdict — the `--dir` CLI entry.
pub fn yardstick_slate_report(
    dir: &str,
    side: &DuelSide,
    seeds_per_tier: u64,
    max_ticks: u64,
    seed_base: u64,
    out: Option<&str>,
) -> Result<()> {
    let slate = run_yardstick_slate(dir, side, seeds_per_tier, max_ticks, seed_base)?;
    println!(
        "\nYARDSTICK SLATE  ·  {}  ·  {} 1v1 maps  ·  {}  ·  {} seeds/tier x 2 seats  ·  cap {}",
        slate.dir,
        slate.per_map.len(),
        slate.profile,
        slate.seeds_per_tier,
        slate.max_ticks
    );
    for map in &slate.per_map {
        let pace = map
            .per_tier
            .iter()
            .filter_map(|t| t.median_victory_tick)
            .max()
            .map_or("no victories".to_string(), |slowest| {
                format!("slowest tier median {slowest}")
            });
        println!(
            "  {:<22} {:>3}/{:<3} ({} unresolved)  ·  {pace}",
            map.scenario, map.wins, map.matches, map.unresolved
        );
    }
    println!("\npooled over the slate:");
    print_tiers(&slate.per_tier);
    println!(
        "total {}/{}  ·  {} unresolved",
        slate.wins, slate.matches, slate.unresolved
    );
    if let Some(path) = out {
        std::fs::write(path, serde_json::to_string_pretty(&slate)?)?;
        println!("raw record: {path}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two seeds, both orientations, a cap far too small to decide:
    /// the plumbing must account for every job and mirror the
    /// personality pairs exactly.
    #[test]
    fn sweep_accounts_for_every_job_and_mirrors_the_swap() {
        let report = run_sweep("skirmish", Level::Medium, 2, 40, 7_000).unwrap();
        assert_eq!(report.matches.len(), 4);
        assert_eq!(report.victories + report.draws + report.undecided, 4);
        for pair in report.matches.chunks(2) {
            assert_eq!(pair[0].seed, pair[1].seed);
            assert!(!pair[0].swapped && pair[1].swapped);
            assert_eq!(pair[0].aggression[0], pair[1].aggression[1]);
            assert_eq!(pair[0].aggression[1], pair[1].aggression[0]);
        }
    }

    /// The duel accounts for every job, and dial overrides show up in
    /// the labels the report carries.
    #[test]
    fn duel_accounts_for_every_job_and_labels_dials() {
        let a = DuelSide {
            level: Level::Medium,
            skill: Some(700),
            cadence: Some(30),
        };
        let b = DuelSide {
            level: Level::Easy,
            skill: None,
            cadence: None,
        };
        let report = run_duel("skirmish", &a, &b, 2, 40, 7_000).unwrap();
        assert_eq!(report.a, "medium/s700/c30");
        assert_eq!(report.b, "easy");
        assert_eq!(
            report.a_wins + report.b_wins + report.draws + report.undecided,
            4
        );
    }

    /// A cap far too small to decide anything: every match must land
    /// in some tier's unresolved column, and a tier with no victories
    /// must report no pace rather than a zero.
    #[test]
    fn the_yardstick_accounts_for_every_tier_and_admits_an_empty_pace() {
        let side = DuelSide {
            level: Level::Medium,
            skill: None,
            cadence: None,
        };
        let report = run_yardstick("skirmish", &side, 1, 40, 3_000).unwrap();
        assert_eq!(report.per_tier.len(), 4);
        assert_eq!(report.matches, 8);
        assert_eq!(report.unresolved, 8);
        for tier in &report.per_tier {
            assert_eq!(tier.matches, 2);
            assert_eq!(tier.wins, 0);
            assert_eq!(tier.median_victory_tick, None);
            assert_eq!(tier.p75_victory_tick, None);
        }
    }

    /// The slate keeps every 1v1 map of the directory, skips the other
    /// formats, and its pooled tier records total the per-map ones.
    #[test]
    fn the_slate_pools_its_maps_and_skips_other_formats() {
        let side = DuelSide {
            level: Level::Medium,
            skill: None,
            cadence: None,
        };
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../scenarios");
        let slate = run_yardstick_slate(dir, &side, 1, 20, 3_000).unwrap();
        assert!(slate.per_map.len() >= 10, "the 1v1 roster is present");
        for map in &slate.per_map {
            assert_eq!(map.matches, 8);
        }
        assert_eq!(slate.matches, 8 * slate.per_map.len() as u32);
        for (t, tier) in slate.per_tier.iter().enumerate() {
            let pooled: u32 = slate.per_map.iter().map(|m| m.per_tier[t].matches).sum();
            assert_eq!(tier.matches, pooled);
        }
    }

    /// Nearest rank, and both quantiles collapse onto a single sample.
    #[test]
    fn quantiles_take_the_nearest_rank() {
        assert_eq!(quantile(&[], 1, 2), None);
        assert_eq!(quantile(&[7], 1, 2), Some(7));
        assert_eq!(quantile(&[7], 3, 4), Some(7));
        assert_eq!(quantile(&[1, 2, 3, 4], 1, 2), Some(3));
        assert_eq!(quantile(&[1, 2, 3, 4], 3, 4), Some(4));
    }
}
