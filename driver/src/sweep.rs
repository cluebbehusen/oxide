//! The decisiveness sweep: N seeds of bot-vs-bot on one 1v1 scenario at
//! one ladder level, each seed played twice — once with complete named
//! profiles resolved by the sim, once with those profiles exchanged between
//! the seats. Where `balance-probe` reads what armies were made of,
//! this reads whether games *end*: decided/undecided counts, seat bias
//! that survives the profile exchange, and decision-tick medians.
//! The 0.12 bot phases gate on it.
//!
//! The exchange swaps the style, variant, aggression, role, and facets as one
//! profile; each seat keeps its own faction and hesitation stream, which is a
//! per-seat variable at any degraded
//! level. A lean that shows up in both orientations is the map or the
//! engine, not profile assignment.
//!
//! `duel` is the sibling instrument: two ladder sides (a resolved rung,
//! optionally with raw skill or cadence dial overrides) fight across
//! N seeds with the sides exchanging seats — the head-to-head the
//! Medium re-metering selects candidates on, since the in-tree ladder
//! test is a tripwire, not a measuring stick.

use anyhow::{Context, Result};
use oxide_sim::bot::{Level, NeuralBot, QuantNet, ResolvedBotProfile, resolve_bot_profiles};
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
    /// Whether the two seats' complete resolved profiles were exchanged.
    pub swapped: bool,
    /// The legacy aggression component of each complete profile, seat-indexed.
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
    /// Mean legacy aggression component of winning profiles over victories.
    pub mean_winner_aggression: Option<f64>,
    /// Mean legacy aggression component of losing profiles over victories.
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
    let mut sc = base.clone();
    sc.seed = seed;
    configure_named_pair(&mut sc, [level; 2]);
    let dealt = resolve_named_pair(&sc)?;
    let profiles = orient_profiles(dealt, swapped);
    let mut state: State = sc.build().context("building scenario")?;
    let mut bots: Vec<NeuralBot> = profiles
        .into_iter()
        .enumerate()
        .map(|(seat, profile)| {
            NeuralBot::ladder_resolved(
                PlayerId(seat as u8),
                seed,
                profile,
                sc.players[seat].faction,
            )
        })
        .collect();
    for _ in 0..max_ticks {
        let mut commands = Vec::new();
        for bot in &mut bots {
            commands.extend(bot.act(&state));
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
    Ok(SweepMatch {
        seed,
        swapped,
        aggression: profiles.map(|profile| profile.aggression),
        ticks: state.current_tick(),
        outcome,
    })
}

/// Configures both seats for the named runtime wrapper while preserving any
/// authored named style, variant, or team role.
pub(crate) fn configure_named_pair(scenario: &mut Scenario, levels: [Level; 2]) {
    for (seat, player) in scenario.players.iter_mut().enumerate() {
        let authored = player.bot_config;
        player.bot = true;
        player.bot_config = Some(BotConfig {
            level: levels[seat],
            aggression: None,
            style: authored.and_then(|config| config.style),
            variant: authored.and_then(|config| config.variant),
            team_role: authored.and_then(|config| config.team_role),
            overseer: false,
        });
    }
}

/// Resolves the exact two profiles the runtime wrapper will seat.
pub(crate) fn resolve_named_pair(scenario: &Scenario) -> Result<[ResolvedBotProfile; 2]> {
    let profiles = resolve_bot_profiles(scenario)
        .with_context(|| format!("resolving bot profiles for {}", scenario.name))?;
    Ok([
        profiles[0].context("seat 0 did not resolve a named profile")?,
        profiles[1].context("seat 1 did not resolve a named profile")?,
    ])
}

/// Exchanges complete resolved profiles without moving either seat's faction,
/// player id, or hesitation stream.
pub(crate) fn orient_profiles(
    profiles: [ResolvedBotProfile; 2],
    swapped: bool,
) -> [ResolvedBotProfile; 2] {
    if swapped {
        [profiles[1], profiles[0]]
    } else {
        profiles
    }
}

/// One side of a duel: a resolved named ladder profile, optionally with a
/// cadence override. Supplying a skill explicitly opts that side into the
/// zero-facet raw diagnostic path whose hesitation derives from that skill.
#[derive(Debug, Clone, Serialize)]
pub struct DuelSide {
    /// The base rung.
    pub level: Level,
    /// Raw skill-knob override (None: the named strategy condition).
    pub skill: Option<u32>,
    /// Cadence override (None: the rung's own).
    pub cadence: Option<u64>,
}

impl DuelSide {
    /// The profile's display name, dials included.
    pub fn label(&self) -> String {
        let mut label = format!("{:?}", self.level).to_lowercase();
        if let Some(skill) = self.skill {
            label.push_str(&format!("/raw-s{skill}"));
        }
        if let Some(cadence) = self.cadence {
            label.push_str(&format!("/c{cadence}"));
        }
        label
    }

    fn bot_with_resolved_profile(
        &self,
        player: PlayerId,
        seed: u64,
        faction: oxide_sim::Faction,
        profile: ResolvedBotProfile,
    ) -> NeuralBot {
        if let Some(skill) = self.skill {
            NeuralBot::with_profile(
                player,
                self.cadence.unwrap_or_else(|| self.level.cadence()),
                QuantNet::ladder().clone(),
                skill,
                profile.aggression,
                faction,
                0,
                seed,
            )
        } else if let Some(cadence) = self.cadence {
            NeuralBot::ladder_resolved_with_net_at_cadence(
                player,
                seed,
                profile,
                faction,
                QuantNet::ladder().clone(),
                cadence,
            )
        } else {
            NeuralBot::ladder_resolved_with_net(
                player,
                seed,
                profile,
                faction,
                QuantNet::ladder().clone(),
            )
        }
    }

    fn raw_bot_with_aggression(
        &self,
        player: PlayerId,
        seed: u64,
        faction: oxide_sim::Faction,
        aggression: u32,
    ) -> NeuralBot {
        if let Some(skill) = self.skill {
            NeuralBot::with_profile(
                player,
                self.cadence.unwrap_or_else(|| self.level.cadence()),
                QuantNet::ladder().clone(),
                skill,
                aggression,
                faction,
                0,
                seed,
            )
        } else if let Some(cadence) = self.cadence {
            NeuralBot::ladder_with_net_at_cadence(
                player,
                seed,
                self.level,
                Some(aggression),
                faction,
                QuantNet::ladder().clone(),
                cadence,
            )
        } else {
            NeuralBot::ladder_with_net(
                player,
                seed,
                self.level,
                Some(aggression),
                faction,
                QuantNet::ladder().clone(),
            )
        }
    }

    fn raw_yardstick_label(&self) -> String {
        format!("{}/raw-a500", self.label())
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
/// Sides carry their level and any explicit raw override when they change
/// chairs. The scenario resolves the complete named profiles for each leg;
/// each physical seat retains its faction and hesitation stream.
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
    let mut levels = [b.level; 2];
    levels[a_seat] = a.level;
    configure_named_pair(&mut sc, levels);
    let profiles = resolve_named_pair(&sc)?;
    let mut state: State = sc.build().context("building scenario")?;
    let mut bots: Vec<NeuralBot> = sc
        .players
        .iter()
        .enumerate()
        .map(|(seat, player)| {
            let side = if seat == a_seat { a } else { b };
            side.bot_with_resolved_profile(
                PlayerId(seat as u8),
                seed,
                player.faction,
                profiles[seat],
            )
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
pub(crate) fn quantile(sorted: &[u64], num: usize, den: usize) -> Option<u64> {
    (!sorted.is_empty()).then(|| sorted[(sorted.len() * num / den).min(sorted.len() - 1)])
}

/// The scripted-yardstick verdict for one explicitly raw zero-facet profile.
/// It fixes aggression at 500 and measures every scripted tier from both
/// seats, over as many seeds as the recalibration wants.
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
        profile: side.raw_yardstick_label(),
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
        let mut bot = side.raw_bot_with_aggression(PlayerId(seat), seed, faction, 500);
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

/// Measures `side` against the four scripted tiers on the explicit raw
/// zero-facet path at aggression 500. This isolates legacy skill/cadence
/// calibration; it is not named-profile or shipped-runtime coverage.
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
        profile: side.raw_yardstick_label(),
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

    fn assert_same_command_source(
        mut actual: NeuralBot,
        mut expected: NeuralBot,
        seed: u64,
        label: &str,
    ) {
        let mut sc = crate::runner::load_scenario("skirmish").unwrap();
        sc.seed = seed;
        let mut state = sc.build().unwrap();
        for _ in 0..400 {
            let actual_commands = actual.act(&state);
            let expected_commands = expected.act(&state);
            assert_eq!(
                actual_commands,
                expected_commands,
                "{label} at tick {}",
                state.current_tick()
            );
            state.tick(&expected_commands);
        }
    }

    /// Two seeds, both orientations, a cap far too small to decide:
    /// the plumbing must account for every job and exchange the reported
    /// legacy aggression components with the profiles.
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
        assert_eq!(report.a, "medium/raw-s700/c30");
        assert_eq!(report.b, "easy");
        assert_eq!(
            report.a_wins + report.b_wins + report.draws + report.undecided,
            4
        );
    }

    /// A side without a raw skill override receives the complete profile
    /// resolved by the same scenario path as an ordinary match.
    #[test]
    fn default_duel_sides_are_exact_resolved_named_profiles() {
        let base = crate::runner::load_scenario("skirmish").unwrap();
        for level in Level::LADDER {
            let side = DuelSide {
                level,
                skill: None,
                cadence: None,
            };
            for seed in [17, 31] {
                let mut scenario = base.clone();
                scenario.seed = seed;
                configure_named_pair(&mut scenario, [level; 2]);
                let profiles = resolve_named_pair(&scenario).unwrap();
                let player = PlayerId(0);
                let faction = scenario.players[0].faction;
                assert_same_command_source(
                    side.bot_with_resolved_profile(player, seed, faction, profiles[0]),
                    NeuralBot::ladder_resolved(player, seed, profiles[0], faction),
                    seed,
                    &format!("{level:?} profile {:?}", profiles[0]),
                );
            }
        }
    }

    /// Cadence-only probes retain named semantics, while an explicit
    /// skill still opts into the historical raw profile used by
    /// re-metering experiments.
    #[test]
    fn duel_side_overrides_keep_their_declared_semantics() {
        let seed = 19;
        let player = PlayerId(0);
        let faction = oxide_sim::Faction::Ferrous;
        let cadence_only = DuelSide {
            level: Level::Hard,
            skill: None,
            cadence: Some(32),
        };
        let mut scenario = crate::runner::load_scenario("skirmish").unwrap();
        scenario.seed = seed;
        configure_named_pair(&mut scenario, [Level::Hard; 2]);
        let profile = resolve_named_pair(&scenario).unwrap()[0];
        assert_same_command_source(
            cadence_only.bot_with_resolved_profile(player, seed, faction, profile),
            NeuralBot::ladder_resolved_with_net_at_cadence(
                player,
                seed,
                profile,
                faction,
                QuantNet::ladder().clone(),
                32,
            ),
            seed,
            "cadence-only named profile",
        );

        let raw = DuelSide {
            level: Level::Medium,
            skill: Some(700),
            cadence: Some(30),
        };
        assert_same_command_source(
            raw.raw_bot_with_aggression(player, seed, faction, 550),
            NeuralBot::with_profile(
                player,
                30,
                QuantNet::ladder().clone(),
                700,
                550,
                faction,
                0,
                seed,
            ),
            seed,
            "explicit raw profile",
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
        assert_eq!(report.profile, "medium/raw-a500");
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
