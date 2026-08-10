//! Composition telemetry: what a match's rosters and combat armies were
//! actually made of, both cost-weighted and body-time-weighted. The
//! historical all-unit lens remains available for economy diagnostics;
//! the parallel combat lens counts only weapon-bearing units, so
//! Harvesters cannot make an army look varied. Value share is the balance
//! review's measuring stick; body-time share catches a cheap unit
//! dominating army presence while expensive specialists make the value
//! mix look healthy.
//!
//! A record also states the terms it was measured under: how the match
//! ended, whether the tick cap ended it, the last sample on which
//! anything moved, and the economy left at the final tick. A capped
//! stalemate's army mix is evidence about a stalemate, not about army
//! choice, and a reader that cannot tell a live war from an exhausted
//! economy draws the wrong conclusion from the same number.
//!
//! Buildings ride beside units because the tech tree is a construction
//! decision: a roster that never stands a Fabricator never had the
//! advanced kinds available to choose, and the unit shares alone cannot
//! say which of the two happened.

use crate::runner;
use anyhow::{Result, ensure};
use oxide_sim::{
    BuildingId, BuildingKind, Event, Faction, GameResult, Scenario, State, TickReport, UnitKind,
};
use std::collections::{BTreeMap, BTreeSet};

/// One seat's economy at the final sampled tick.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct SeatEconomy {
    /// Whether the seat conceded and can no longer act or recover.
    pub resigned: bool,
    /// Whether this active seat can currently receive recurring automatic
    /// scrap from a Reclaimer or either Foundry recovery channel.
    pub recovery_income_active: bool,
    /// Spendable scrap in the bank.
    pub bank_scrap: u32,
    /// Harvesters still alive in the world.
    pub living_harvesters: u32,
    /// Paid Harvesters still waiting in living, completed producers.
    pub queued_harvesters: u32,
    /// Living, completed Reclaimers.
    pub completed_reclaimers: u32,
    /// Living Foundries.
    pub living_foundries: u32,
}

/// Resource state at the final tick, used to classify capped matches.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct FinalEconomy {
    /// Harvestable scrap still on the map: untouched node scrap plus
    /// battlefield wreck salvage. Banked and carried scrap are excluded.
    pub remaining_map_salvage: u64,
    /// Per-seat resource and recovery capacity.
    pub seats: Vec<SeatEconomy>,
}

/// Meaningful work observed over the match.
///
/// Tick caps are measurement boundaries, not verdicts. Recent combat or
/// economic events identify a long active match; old timestamps alongside
/// exhausted recovery capacity identify a genuine stall.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct MatchActivity {
    /// Most recent attack hit or turret shot.
    pub last_combat_tick: u64,
    /// Most recent deposit, production, construction, salvage, or depletion.
    pub last_economy_tick: u64,
    /// Unit weapon hits observed.
    pub attack_hits: u64,
    /// Turret shots observed.
    pub turret_shots: u64,
    /// Artillery shells launched.
    pub shell_shots: u64,
    /// Scrap deliveries observed.
    pub deliveries: u64,
    /// Scrap delivered over the match.
    pub delivered_scrap: u64,
    /// Units completed over the match.
    pub units_trained: u64,
    /// Buildings completed over the match.
    pub buildings_completed: u64,
}

/// One match's cost-weighted composition, per seat.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MatchComposition {
    /// Scenario name.
    pub scenario: String,
    /// The map's declared pace label — its size class, and the cohort
    /// key for "does this skew belong to one map class". Empty when the
    /// scenario carries no metadata.
    pub pace: String,
    /// Seed the match ran under.
    pub seed: u64,
    /// Per seat: unit-kind name -> integrated cost-share of that seat's
    /// whole roster over the sampled match (shares sum to 1 per seat
    /// that ever fielded a unit). Includes Harvesters for compatibility
    /// and economy diagnostics.
    pub seats: Vec<BTreeMap<String, f64>>,
    /// Per seat: Shannon entropy (bits) of that seat's OWN mix. Two
    /// seats each spamming a different kind average to a mix that looks
    /// diverse; only the per-seat figures show the spam.
    pub entropy_bits: Vec<f64>,
    /// Per seat: unit-kind name -> integrated body-count share over the
    /// sampled match. Every living unit adds one per sample regardless
    /// of purchase price. Includes Harvesters.
    pub count_seats: Vec<BTreeMap<String, f64>>,
    /// Per seat: Shannon entropy (bits) of its body-time mix.
    pub count_entropy_bits: Vec<f64>,
    /// Per seat: weapon-bearing unit-kind name -> integrated cost-share
    /// over that seat's competitive lifetime. A seat contributes only
    /// while it is not resigned and holds a living completed Foundry,
    /// so its pre-defeat play remains evidence but autonomous remnants
    /// do not. Economy-only units are excluded.
    pub combat_seats: Vec<BTreeMap<String, f64>>,
    /// Per seat: Shannon entropy (bits) of its combat value mix.
    pub combat_entropy_bits: Vec<f64>,
    /// Per seat: weapon-bearing unit-kind name -> integrated body-count
    /// share over that seat's competitive lifetime.
    pub combat_count_seats: Vec<BTreeMap<String, f64>>,
    /// Per seat: Shannon entropy (bits) of its combat body-time mix.
    pub combat_count_entropy_bits: Vec<f64>,
    /// Per seat: building-kind name -> distinct buildings of that kind
    /// seen standing built at any sample. A rebuild after a loss counts
    /// twice (ids never repeat) and a site razed before it finished
    /// counts not at all — the number answers "what did this seat
    /// finish", which is what the tech tree gates on.
    pub buildings: Vec<BTreeMap<String, u32>>,
    /// Per seat: completed buildings first observed while the seat was
    /// still competitive. This is the reach lens used by promotion;
    /// an autonomous remnant cannot finish a project into credit.
    pub competitive_buildings: Vec<BTreeMap<String, u32>>,
    /// Per seat: the roster it played, lowercased.
    pub factions: Vec<String>,
    /// Ticks the match ran before a result (or the cap).
    pub ticks: u64,
    /// How the match ended; `None` when the tick cap ended it.
    pub result: Option<GameResult>,
    /// Whether the tick cap ended the match. Carried explicitly because
    /// inferring it from `ticks == max_ticks` is wrong on a match
    /// decided on its final tick.
    pub capped: bool,
    /// Winning seats — empty on a draw and on a capped match.
    pub winners: Vec<u8>,
    /// Economy and recovery capacity at the final tick. This distinguishes
    /// an active long game from a cap caused by exhausted salvage or a
    /// destroyed harvest line.
    pub final_economy: FinalEconomy,
    /// Combat and economy event totals plus their most recent ticks.
    pub activity: MatchActivity,
    /// The last sample at which any seat's army value or standing
    /// building count changed. Sample-resolution, so a frozen tail
    /// reads as `ticks - last_progress_tick`.
    pub last_progress_tick: u64,
}

/// Runs one bot-vs-bot match and integrates each seat's army value by
/// kind: every sample adds `cost` for each living unit, so a unit's
/// share reflects both how many were fielded and how long they lived —
/// the honest "what was this army made of" number.
pub fn sample_match(
    scenario: &Scenario,
    max_ticks: u64,
    sample_every: u64,
) -> Result<MatchComposition> {
    let mut bots = oxide_sim::bot::seat_bots(scenario);
    sample_driven(scenario, max_ticks, sample_every, |state| {
        runner::step(state, &mut bots, None)
    })
}

/// Like [`sample_match`], but the caller drives each tick — how a
/// candidate weights artifact (not yet embedded) gets probed.
pub fn sample_driven(
    scenario: &Scenario,
    max_ticks: u64,
    sample_every: u64,
    mut tick_fn: impl FnMut(&mut oxide_sim::State) -> TickReport,
) -> Result<MatchComposition> {
    ensure!(sample_every > 0, "sample stride must be greater than zero");
    let mut state = scenario.build()?;
    let seats = scenario.players.len();
    let mut value_acc: Vec<BTreeMap<UnitKind, u64>> = vec![BTreeMap::new(); seats];
    let mut count_acc: Vec<BTreeMap<UnitKind, u64>> = vec![BTreeMap::new(); seats];
    let mut combat_value_acc: Vec<BTreeMap<UnitKind, u64>> = vec![BTreeMap::new(); seats];
    let mut combat_count_acc: Vec<BTreeMap<UnitKind, u64>> = vec![BTreeMap::new(); seats];
    let mut standing: Vec<BTreeSet<(BuildingKind, BuildingId)>> = vec![BTreeSet::new(); seats];
    let mut competitive_standing: Vec<BTreeSet<(BuildingKind, BuildingId)>> =
        vec![BTreeSet::new(); seats];
    let mut previous: Vec<(u64, usize)> = vec![(0, 0); seats];
    let mut previous_banks: Vec<u32> = state.players().iter().map(|player| player.scrap).collect();
    let mut last_progress_tick = 0;
    let mut activity = MatchActivity::default();
    let mut ran = 0;
    for tick in 0..max_ticks {
        let report = tick_fn(&mut state);
        note_activity(&mut activity, &report);
        ran = tick + 1;
        let banks: Vec<u32> = state.players().iter().map(|player| player.scrap).collect();
        // Spending is meaningful economic work even when the purchased
        // unit or structure has not completed yet. Passive Foundry and
        // Reclaimer credits alone do not excuse an otherwise frozen
        // match; once the policy uses that income, the bank decrease
        // records the progress.
        if banks
            .iter()
            .zip(&previous_banks)
            .any(|(current, previous)| current < previous)
        {
            activity.last_economy_tick = ran;
        }
        previous_banks = banks;
        if tick % sample_every == 0 {
            let mut live: Vec<(u64, usize)> = vec![(0, 0); seats];
            let competitive: Vec<bool> = (0..seats)
                .map(|seat| {
                    !state.players()[seat].resigned
                        && state.buildings().iter().any(|building| {
                            building.player.0 as usize == seat
                                && building.hp > 0
                                && building.built
                                && building.kind == BuildingKind::Foundry
                        })
                })
                .collect();
            for unit in state.units() {
                let seat = unit.player.0 as usize;
                if seat < seats {
                    let cost = u64::from(unit.kind.stats().cost);
                    *value_acc[seat].entry(unit.kind).or_default() += cost;
                    *count_acc[seat].entry(unit.kind).or_default() += 1;
                    if competitive[seat] && unit.kind.stats().can_fight() {
                        *combat_value_acc[seat].entry(unit.kind).or_default() += cost;
                        *combat_count_acc[seat].entry(unit.kind).or_default() += 1;
                    }
                    live[seat].0 += cost;
                }
            }
            for building in state.buildings() {
                let seat = building.player.0 as usize;
                if seat < seats {
                    live[seat].1 += 1;
                    if building.built {
                        standing[seat].insert((building.kind, building.id));
                        if competitive[seat] {
                            competitive_standing[seat].insert((building.kind, building.id));
                        }
                    }
                }
            }
            if live != previous {
                last_progress_tick = ran;
                previous = live;
            }
        }
        if state.result().is_some() {
            break;
        }
    }
    let normalize = |kinds: BTreeMap<UnitKind, u64>| {
        let total: u64 = kinds.values().sum();
        kinds
            .into_iter()
            .map(|(kind, value)| {
                (
                    kind.name().to_string(),
                    if total == 0 {
                        0.0
                    } else {
                        value as f64 / total as f64
                    },
                )
            })
            .collect()
    };
    let shares: Vec<BTreeMap<String, f64>> = value_acc.into_iter().map(&normalize).collect();
    let entropy_bits = shares.iter().map(|seat| entropy(seat.values())).collect();
    let count_seats: Vec<BTreeMap<String, f64>> = count_acc.into_iter().map(normalize).collect();
    let count_entropy_bits = count_seats
        .iter()
        .map(|seat| entropy(seat.values()))
        .collect();
    let combat_seats: Vec<BTreeMap<String, f64>> =
        combat_value_acc.into_iter().map(&normalize).collect();
    let combat_entropy_bits = combat_seats
        .iter()
        .map(|seat| entropy(seat.values()))
        .collect();
    let combat_count_seats: Vec<BTreeMap<String, f64>> =
        combat_count_acc.into_iter().map(normalize).collect();
    let combat_count_entropy_bits = combat_count_seats
        .iter()
        .map(|seat| entropy(seat.values()))
        .collect();
    let buildings = standing
        .into_iter()
        .map(|kinds| {
            let mut counts: BTreeMap<String, u32> = BTreeMap::new();
            for (kind, _) in kinds {
                *counts.entry(kind.name().to_string()).or_default() += 1;
            }
            counts
        })
        .collect();
    let competitive_buildings = competitive_standing
        .into_iter()
        .map(|kinds| {
            let mut counts: BTreeMap<String, u32> = BTreeMap::new();
            for (kind, _) in kinds {
                *counts.entry(kind.name().to_string()).or_default() += 1;
            }
            counts
        })
        .collect();
    let result = state.result();
    let final_economy = capture_final_economy(&state);
    Ok(MatchComposition {
        scenario: scenario.name.clone(),
        pace: scenario
            .meta
            .as_ref()
            .map_or_else(String::new, |meta| meta.pace.clone()),
        seed: scenario.seed,
        seats: shares,
        entropy_bits,
        count_seats,
        count_entropy_bits,
        combat_seats,
        combat_entropy_bits,
        combat_count_seats,
        combat_count_entropy_bits,
        buildings,
        competitive_buildings,
        factions: state
            .players()
            .iter()
            .map(|p| faction_name(p.faction).to_string())
            .collect(),
        ticks: ran,
        result,
        capped: result.is_none(),
        winners: state.winners().iter().map(|p| p.0).collect(),
        final_economy,
        activity,
        last_progress_tick,
    })
}

fn note_activity(activity: &mut MatchActivity, report: &TickReport) {
    let tick = report.tick.saturating_add(1);
    for event in &report.events {
        match event {
            Event::AttackHit { .. } => {
                activity.attack_hits += 1;
                activity.last_combat_tick = tick;
            }
            Event::TurretFired { .. } => {
                activity.turret_shots += 1;
                activity.last_combat_tick = tick;
            }
            Event::ShellLaunched { .. } => {
                activity.shell_shots += 1;
                activity.last_combat_tick = tick;
            }
            Event::ScrapDeposited { amount, .. } => {
                activity.deliveries += 1;
                activity.delivered_scrap += u64::from(*amount);
                activity.last_economy_tick = tick;
            }
            Event::UnitTrained { .. } => {
                activity.units_trained += 1;
                activity.last_economy_tick = tick;
            }
            Event::BuildingCompleted { .. } => {
                activity.buildings_completed += 1;
                activity.last_economy_tick = tick;
            }
            Event::BuildingSalvaged { .. }
            | Event::BuildCancelled { .. }
            | Event::NodeDepleted { .. } => {
                activity.last_economy_tick = tick;
            }
            Event::UnitDied { .. }
            | Event::BuildingDestroyed { .. }
            | Event::ChargeDetonated { .. } => {
                activity.last_combat_tick = tick;
            }
            Event::UnitBoarded { .. } | Event::UnitUnloaded { .. } => {
                activity.last_economy_tick = tick;
            }
            Event::ShellLanded { .. }
            | Event::UnitRepaired { .. }
            | Event::CommandRejected { .. }
            | Event::OrderStalled { .. }
            | Event::GameOver { .. }
            | Event::PlayerResigned { .. } => {}
        }
    }
}

fn capture_final_economy(state: &State) -> FinalEconomy {
    let mut seats: Vec<SeatEconomy> = state
        .players()
        .iter()
        .enumerate()
        .map(|(index, player)| SeatEconomy {
            resigned: player.resigned,
            recovery_income_active: state.recovery_income_active(oxide_sim::PlayerId(index as u8)),
            bank_scrap: player.scrap,
            ..SeatEconomy::default()
        })
        .collect();

    for unit in state.units() {
        if unit.hp > 0
            && unit.kind == UnitKind::Harvester
            && let Some(seat) = seats.get_mut(unit.player.0 as usize)
        {
            seat.living_harvesters += 1;
        }
    }
    for building in state.buildings() {
        if building.hp == 0 || !building.built {
            continue;
        }
        let Some(seat) = seats.get_mut(building.player.0 as usize) else {
            continue;
        };
        seat.queued_harvesters += u32::try_from(
            building
                .queue
                .iter()
                .filter(|&&kind| kind == UnitKind::Harvester)
                .count(),
        )
        .expect("the bounded production queue fits in u32");
        match building.kind {
            BuildingKind::Reclaimer => seat.completed_reclaimers += 1,
            BuildingKind::Foundry => seat.living_foundries += 1,
            _ => {}
        }
    }

    let mut remaining_map_salvage = 0u64;
    for y in 0..state.map().height() {
        for x in 0..state.map().width() {
            let tile = state
                .map()
                .tile(chassis::grid::TilePos::new(x, y))
                .expect("coordinates are inside the map");
            remaining_map_salvage += u64::from(tile.scrap) + u64::from(tile.wreck);
        }
    }
    FinalEconomy {
        remaining_map_salvage,
        seats,
    }
}

/// The roster label the cohort keys speak.
fn faction_name(faction: Faction) -> &'static str {
    match faction {
        Faction::Ferrous => "ferrous",
        Faction::Cupric => "cupric",
    }
}

/// Shannon entropy (bits) of a share series. Zero shares contribute
/// nothing; the series is assumed normalized.
fn entropy<'a>(shares: impl Iterator<Item = &'a f64>) -> f64 {
    -shares
        .filter(|&&p| p > 0.0)
        .map(|p| p * p.log2())
        .sum::<f64>()
}

/// Nearest-rank quantile over an already-sorted series.
fn quantile(sorted: &[f64], num: usize, den: usize) -> f64 {
    let rank = (sorted.len() * num).div_ceil(den);
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

/// How the per-seat entropies are spread. The mean of a cohort's seats
/// answers "was there variety on average"; `p10` answers "did anyone
/// spam", which is the question a mean cannot be made to answer.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EntropySpread {
    /// Mean per-seat entropy (bits).
    pub mean: f64,
    /// Tenth-percentile seat — the spam detector.
    pub p10: f64,
    /// First-quartile seat.
    pub p25: f64,
    /// Median seat.
    pub median: f64,
}

/// How dominant each seat's most common body was. The upper tail catches
/// individual armies that a mean composition can hide.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DominanceSpread {
    /// Mean of each seat's largest unit-count share.
    pub mean: f64,
    /// Ninetieth-percentile largest share.
    pub p90: f64,
    /// Largest share observed on any seat.
    pub max: f64,
}

/// Aggregates seat compositions across matches under both value and
/// body-time lenses. A roster collapsed onto one kind scores zero
/// entropy; an even spread over eight kinds scores three bits.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Aggregate {
    /// Mean cost-share per kind name across every sampled seat.
    pub mean_share: BTreeMap<String, f64>,
    /// Shannon entropy (bits) of the mean mix.
    pub entropy_bits: f64,
    /// The per-seat entropy distribution; `None` when no seat in the
    /// cohort ever fielded a unit.
    pub seat_entropy: Option<EntropySpread>,
    /// Mean body-time share per kind name across every sampled seat.
    pub mean_count_share: BTreeMap<String, f64>,
    /// Shannon entropy (bits) of the mean body-time mix.
    pub count_entropy_bits: f64,
    /// Per-seat body-time entropy distribution.
    pub seat_count_entropy: Option<EntropySpread>,
    /// Distribution of the largest body-time share on each seat.
    pub seat_count_dominance: Option<DominanceSpread>,
    /// Mean combat cost-share per kind across seats that fielded an
    /// army during their competitive lifetime.
    pub mean_combat_share: BTreeMap<String, f64>,
    /// Shannon entropy (bits) of the mean competitive combat mix.
    pub combat_entropy_bits: f64,
    /// Per-seat competitive combat value-entropy distribution.
    pub seat_combat_entropy: Option<EntropySpread>,
    /// Mean competitive combat body-time share per kind.
    pub mean_combat_count_share: BTreeMap<String, f64>,
    /// Shannon entropy (bits) of the mean competitive combat body mix.
    pub combat_count_entropy_bits: f64,
    /// Per-seat competitive combat body-time entropy distribution.
    pub seat_combat_count_entropy: Option<EntropySpread>,
    /// Distribution of each competitive combat seat's largest
    /// body-time share.
    pub seat_combat_count_dominance: Option<DominanceSpread>,
    /// Mean count of finished buildings per seat, by kind.
    pub mean_buildings: BTreeMap<String, f64>,
    /// Share of seats that finished at least one of that kind — the
    /// tech-climb reading a mean count blurs.
    pub seats_with_building: BTreeMap<String, f64>,
    /// Mean count of buildings completed during competitive lifetimes,
    /// across seats that fielded a competitive combat army.
    pub competitive_mean_buildings: BTreeMap<String, f64>,
    /// Share of competitive combat seats that completed at least one of
    /// that kind before resignation or Foundry loss.
    pub competitive_seats_with_building: BTreeMap<String, f64>,
    /// Seats aggregated.
    pub seats: usize,
    /// Aggregated seats that fielded a weapon-bearing unit while still
    /// competitive.
    pub combat_seats: usize,
    /// Matches contributing at least one seat.
    pub matches: usize,
    /// Contributing matches that reached a result.
    pub decided: usize,
    /// Contributing matches the tick cap ended.
    pub capped: usize,
}

/// Folds match compositions into one mean mix.
pub fn aggregate(matches: &[MatchComposition]) -> Aggregate {
    aggregate_where(matches, |_, _| true)
}

/// Folds the seats a predicate keeps. A seat that never fielded a unit
/// is skipped whatever the predicate says — it has no mix to report —
/// and a match contributes to the counts only through the seats kept.
pub fn aggregate_where(
    matches: &[MatchComposition],
    keep: impl Fn(&MatchComposition, usize) -> bool,
) -> Aggregate {
    let mut value_sum: BTreeMap<String, f64> = BTreeMap::new();
    let mut count_sum: BTreeMap<String, f64> = BTreeMap::new();
    let mut combat_value_sum: BTreeMap<String, f64> = BTreeMap::new();
    let mut combat_count_sum: BTreeMap<String, f64> = BTreeMap::new();
    let mut building_sum: BTreeMap<String, u32> = BTreeMap::new();
    let mut building_seats: BTreeMap<String, usize> = BTreeMap::new();
    let mut competitive_building_sum: BTreeMap<String, u32> = BTreeMap::new();
    let mut competitive_building_seats: BTreeMap<String, usize> = BTreeMap::new();
    let mut entropies: Vec<f64> = Vec::new();
    let mut count_entropies: Vec<f64> = Vec::new();
    let mut count_dominance: Vec<f64> = Vec::new();
    let mut combat_entropies: Vec<f64> = Vec::new();
    let mut combat_count_entropies: Vec<f64> = Vec::new();
    let mut combat_count_dominance: Vec<f64> = Vec::new();
    let mut seats = 0usize;
    let mut combat_seats = 0usize;
    let (mut played, mut decided, mut capped) = (0usize, 0usize, 0usize);
    for m in matches {
        let mut contributed = false;
        for (seat, mix) in m.seats.iter().enumerate() {
            if mix.is_empty() || !keep(m, seat) {
                continue;
            }
            contributed = true;
            seats += 1;
            for (kind, share) in mix {
                *value_sum.entry(kind.clone()).or_default() += share;
            }
            entropies.push(m.entropy_bits.get(seat).copied().unwrap_or_default());
            if let Some(count_mix) = m.count_seats.get(seat) {
                for (kind, share) in count_mix {
                    *count_sum.entry(kind.clone()).or_default() += share;
                }
                count_entropies.push(m.count_entropy_bits.get(seat).copied().unwrap_or_default());
                count_dominance.push(
                    count_mix
                        .values()
                        .copied()
                        .max_by(f64::total_cmp)
                        .unwrap_or_default(),
                );
            }
            if let Some(combat_mix) = m.combat_seats.get(seat)
                && !combat_mix.is_empty()
            {
                combat_seats += 1;
                for (kind, share) in combat_mix {
                    *combat_value_sum.entry(kind.clone()).or_default() += share;
                }
                combat_entropies.push(m.combat_entropy_bits.get(seat).copied().unwrap_or_default());
                if let Some(combat_count_mix) = m.combat_count_seats.get(seat) {
                    for (kind, share) in combat_count_mix {
                        *combat_count_sum.entry(kind.clone()).or_default() += share;
                    }
                    combat_count_entropies.push(
                        m.combat_count_entropy_bits
                            .get(seat)
                            .copied()
                            .unwrap_or_default(),
                    );
                    combat_count_dominance.push(
                        combat_count_mix
                            .values()
                            .copied()
                            .max_by(f64::total_cmp)
                            .unwrap_or_default(),
                    );
                }
                for (kind, count) in m.competitive_buildings.get(seat).into_iter().flatten() {
                    *competitive_building_sum.entry(kind.clone()).or_default() += count;
                    *competitive_building_seats.entry(kind.clone()).or_default() += 1;
                }
            }
            for (kind, count) in m.buildings.get(seat).into_iter().flatten() {
                *building_sum.entry(kind.clone()).or_default() += count;
                *building_seats.entry(kind.clone()).or_default() += 1;
            }
        }
        if contributed {
            played += 1;
            if m.capped {
                capped += 1;
            } else {
                decided += 1;
            }
        }
    }
    let per_seat = |v: f64| if seats == 0 { 0.0 } else { v / seats as f64 };
    let per_combat_seat = |v: f64| {
        if combat_seats == 0 {
            0.0
        } else {
            v / combat_seats as f64
        }
    };
    let mean_share: BTreeMap<String, f64> = value_sum
        .into_iter()
        .map(|(k, v)| (k, per_seat(v)))
        .collect();
    let entropy_bits = entropy(mean_share.values());
    let mean_count_share: BTreeMap<String, f64> = count_sum
        .into_iter()
        .map(|(k, v)| (k, per_seat(v)))
        .collect();
    let count_entropy_bits = entropy(mean_count_share.values());
    let mean_combat_share: BTreeMap<String, f64> = combat_value_sum
        .into_iter()
        .map(|(k, v)| (k, per_combat_seat(v)))
        .collect();
    let combat_entropy_bits = entropy(mean_combat_share.values());
    let mean_combat_count_share: BTreeMap<String, f64> = combat_count_sum
        .into_iter()
        .map(|(k, v)| (k, per_combat_seat(v)))
        .collect();
    let combat_count_entropy_bits = entropy(mean_combat_count_share.values());
    entropies.sort_by(f64::total_cmp);
    count_entropies.sort_by(f64::total_cmp);
    count_dominance.sort_by(f64::total_cmp);
    combat_entropies.sort_by(f64::total_cmp);
    combat_count_entropies.sort_by(f64::total_cmp);
    combat_count_dominance.sort_by(f64::total_cmp);
    let spread = |values: &[f64]| {
        (!values.is_empty()).then(|| EntropySpread {
            mean: values.iter().sum::<f64>() / values.len() as f64,
            p10: quantile(values, 1, 10),
            p25: quantile(values, 1, 4),
            median: quantile(values, 1, 2),
        })
    };
    let seat_entropy = spread(&entropies);
    let seat_count_entropy = spread(&count_entropies);
    let seat_count_dominance = (!count_dominance.is_empty()).then(|| DominanceSpread {
        mean: count_dominance.iter().sum::<f64>() / count_dominance.len() as f64,
        p90: quantile(&count_dominance, 9, 10),
        max: *count_dominance.last().expect("non-empty by construction"),
    });
    let seat_combat_entropy = spread(&combat_entropies);
    let seat_combat_count_entropy = spread(&combat_count_entropies);
    let seat_combat_count_dominance =
        (!combat_count_dominance.is_empty()).then(|| DominanceSpread {
            mean: combat_count_dominance.iter().sum::<f64>() / combat_count_dominance.len() as f64,
            p90: quantile(&combat_count_dominance, 9, 10),
            max: *combat_count_dominance
                .last()
                .expect("non-empty by construction"),
        });
    Aggregate {
        mean_share,
        entropy_bits,
        seat_entropy,
        mean_count_share,
        count_entropy_bits,
        seat_count_entropy,
        seat_count_dominance,
        mean_combat_share,
        combat_entropy_bits,
        seat_combat_entropy,
        mean_combat_count_share,
        combat_count_entropy_bits,
        seat_combat_count_entropy,
        seat_combat_count_dominance,
        mean_buildings: building_sum
            .into_iter()
            .map(|(k, v)| (k, per_seat(f64::from(v))))
            .collect(),
        seats_with_building: building_seats
            .into_iter()
            .map(|(k, v)| (k, per_seat(v as f64)))
            .collect(),
        competitive_mean_buildings: competitive_building_sum
            .into_iter()
            .map(|(k, v)| (k, per_combat_seat(f64::from(v))))
            .collect(),
        competitive_seats_with_building: competitive_building_seats
            .into_iter()
            .map(|(k, v)| (k, per_combat_seat(v as f64)))
            .collect(),
        seats,
        combat_seats,
        matches: played,
        decided,
        capped,
    }
}

/// Splits the seats into cohorts by a key, then aggregates each. A seat
/// keyed `None` lands in no cohort; an empty cohort is not reported.
pub fn aggregate_by(
    matches: &[MatchComposition],
    key: impl Fn(&MatchComposition, usize) -> Option<String>,
) -> BTreeMap<String, Aggregate> {
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for m in matches {
        for seat in 0..m.seats.len() {
            keys.extend(key(m, seat));
        }
    }
    keys.into_iter()
        .map(|k| {
            let agg = aggregate_where(matches, |m, seat| key(m, seat).as_deref() == Some(&k));
            (k, agg)
        })
        .collect()
}

/// Cohort key: the roster the seat played.
pub fn by_faction(m: &MatchComposition, seat: usize) -> Option<String> {
    m.factions.get(seat).cloned()
}

/// Cohort key: the map's declared size class.
pub fn by_pace(m: &MatchComposition, _seat: usize) -> Option<String> {
    (!m.pace.is_empty()).then(|| m.pace.clone())
}

/// Cohort key: whether the match reached a result or the cap ended it.
pub fn by_outcome(m: &MatchComposition, _seat: usize) -> Option<String> {
    Some(if m.capped { "capped" } else { "decided" }.to_string())
}

/// Cohort key: the map.
pub fn by_scenario(m: &MatchComposition, _seat: usize) -> Option<String> {
    Some(m.scenario.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a record with everything but the mixes at their neutral
    /// values, so a test states only the fact it is about.
    fn record(seats: Vec<BTreeMap<String, f64>>) -> MatchComposition {
        let entropy_bits: Vec<f64> = seats.iter().map(|s| entropy(s.values())).collect();
        let count_seats = seats.clone();
        let count_entropy_bits = entropy_bits.clone();
        let combat_seats = seats.clone();
        let combat_entropy_bits = entropy_bits.clone();
        let combat_count_seats = count_seats.clone();
        let combat_count_entropy_bits = count_entropy_bits.clone();
        let seat_count = seats.len();
        MatchComposition {
            scenario: "x".into(),
            pace: String::new(),
            seed: 0,
            buildings: vec![BTreeMap::new(); seats.len()],
            competitive_buildings: vec![BTreeMap::new(); seats.len()],
            factions: vec!["ferrous".into(), "cupric".into()],
            entropy_bits,
            seats,
            count_seats,
            count_entropy_bits,
            combat_seats,
            combat_entropy_bits,
            combat_count_seats,
            combat_count_entropy_bits,
            ticks: 1,
            result: None,
            capped: true,
            winners: Vec::new(),
            final_economy: FinalEconomy {
                remaining_map_salvage: 0,
                seats: vec![SeatEconomy::default(); seat_count],
            },
            activity: MatchActivity::default(),
            last_progress_tick: 0,
        }
    }

    fn mix(shares: &[(&str, f64)]) -> BTreeMap<String, f64> {
        shares.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
    }

    #[test]
    fn a_zero_sample_stride_is_rejected() {
        let scenario = Scenario::skirmish();
        let error = sample_driven(&scenario, 10, 0, |state| state.tick(&[]))
            .expect_err("zero would panic at the modulo operation");
        assert_eq!(error.to_string(), "sample stride must be greater than zero");
    }

    #[test]
    fn a_sampled_skirmish_reports_normalized_shares() {
        let mut scenario = Scenario::skirmish();
        for p in scenario.players.iter_mut() {
            p.bot = true;
        }
        let m = sample_match(&scenario, 600, 20).expect("samples");
        assert_eq!(m.seats.len(), 2);
        for seat in &m.seats {
            if seat.is_empty() {
                continue;
            }
            let total: f64 = seat.values().sum();
            assert!((total - 1.0).abs() < 1e-9, "shares sum to one, got {total}");
        }
        for seat in &m.count_seats {
            if seat.is_empty() {
                continue;
            }
            let total: f64 = seat.values().sum();
            assert!(
                (total - 1.0).abs() < 1e-9,
                "count shares sum to one, got {total}"
            );
        }
        for seat in &m.combat_seats {
            if seat.is_empty() {
                continue;
            }
            let total: f64 = seat.values().sum();
            assert!(
                (total - 1.0).abs() < 1e-9,
                "combat shares sum to one, got {total}"
            );
            assert!(!seat.contains_key("harvester"));
        }
        for seat in &m.combat_count_seats {
            if seat.is_empty() {
                continue;
            }
            let total: f64 = seat.values().sum();
            assert!(
                (total - 1.0).abs() < 1e-9,
                "combat count shares sum to one, got {total}"
            );
            assert!(!seat.contains_key("harvester"));
        }
    }

    #[test]
    fn final_economy_reports_salvage_and_each_seats_recovery_capacity() {
        let mut scenario = Scenario::skirmish();
        scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
            player: 0,
            kind: BuildingKind::Reclaimer,
            x: 10,
            y: 4,
        });
        let m = sample_driven(&scenario, 0, 20, |_| {
            unreachable!("a zero-tick sample never calls the driver")
        })
        .expect("samples");
        assert_eq!(m.final_economy.remaining_map_salvage, 6_400);
        assert_eq!(
            m.final_economy.seats,
            vec![
                SeatEconomy {
                    resigned: false,
                    recovery_income_active: true,
                    bank_scrap: 150,
                    living_harvesters: 3,
                    queued_harvesters: 0,
                    completed_reclaimers: 1,
                    living_foundries: 1,
                },
                SeatEconomy {
                    resigned: false,
                    recovery_income_active: true,
                    bank_scrap: 150,
                    living_harvesters: 3,
                    queued_harvesters: 0,
                    completed_reclaimers: 0,
                    living_foundries: 1,
                },
            ]
        );
    }

    #[test]
    fn final_economy_counts_a_paid_harvester_still_in_production() {
        let scenario = Scenario::skirmish();
        let m = sample_driven(&scenario, 1, 20, |state| {
            let foundry = state
                .buildings()
                .iter()
                .find(|building| {
                    building.player == oxide_sim::PlayerId(0)
                        && building.kind == BuildingKind::Foundry
                })
                .expect("seat zero has a Foundry")
                .id;
            state.tick(&[oxide_sim::PlayerCommand {
                player: oxide_sim::PlayerId(0),
                command: oxide_sim::Command::Train {
                    building: foundry,
                    kind: UnitKind::Harvester,
                },
            }])
        })
        .expect("samples");
        let seat = &m.final_economy.seats[0];
        assert_eq!(seat.living_harvesters, 3);
        assert_eq!(seat.queued_harvesters, 1);
        assert_eq!(
            seat.bank_scrap,
            150 - UnitKind::Harvester.stats().cost,
            "queued units were already paid for"
        );
    }

    #[test]
    fn final_economy_reports_fast_foundry_recovery_from_sim_truth() {
        let mut scenario = Scenario::skirmish();
        scenario.players[0].scrap = 0;
        scenario
            .units
            .retain(|unit| unit.player != 0 || unit.kind != UnitKind::Harvester);
        let m = sample_driven(&scenario, 0, 20, |_| {
            unreachable!("a zero-tick sample never calls the driver")
        })
        .expect("samples");
        assert!(
            m.final_economy.seats[0].recovery_income_active,
            "a stranded seat with a living Foundry has fast automatic recovery"
        );
        assert!(
            m.final_economy.seats[1].recovery_income_active,
            "the Foundry drip means every unresigned Foundry seat reports passive income"
        );
    }

    #[test]
    fn activity_distinguishes_recent_economy_and_combat_from_roster_churn() {
        let mut activity = MatchActivity::default();
        note_activity(
            &mut activity,
            &TickReport {
                tick: 41,
                events: vec![
                    Event::ScrapDeposited {
                        player: oxide_sim::PlayerId(0),
                        amount: 17,
                    },
                    Event::TurretFired {
                        turret: BuildingId(3),
                        kind: BuildingKind::Turret,
                        target: oxide_sim::Target::Unit(oxide_sim::UnitId(4)),
                        turret_pos: chassis::fx::Vec2Fx::ZERO,
                        target_pos: chassis::fx::Vec2Fx::ZERO,
                    },
                ],
            },
        );
        assert_eq!(
            activity,
            MatchActivity {
                last_combat_tick: 42,
                last_economy_tick: 42,
                turret_shots: 1,
                shell_shots: 0,
                deliveries: 1,
                delivered_scrap: 17,
                ..MatchActivity::default()
            }
        );
    }

    #[test]
    fn passive_bank_income_alone_does_not_excuse_a_stall() {
        let scenario = Scenario::skirmish();
        let m = sample_driven(
            &scenario,
            oxide_sim::stats::FOUNDRY_DRIP_START_TICK,
            20,
            |state| state.tick(&[]),
        )
        .expect("samples the Foundry's drip credit");
        assert_eq!(m.activity.last_economy_tick, 0);
        assert_eq!(m.activity.deliveries, 0);
        assert_eq!(m.activity.units_trained, 0);
        assert!(
            m.final_economy
                .seats
                .iter()
                .all(|seat| seat.recovery_income_active),
            "the Rust economy reports the Foundry drip without Python constants"
        );
    }

    /// The terms of the measurement: an undecided match says so, names
    /// no winner, and reports where the last change happened. Every
    /// seat starts with a Foundry, so the tech reading is a real count
    /// from tick one.
    #[test]
    fn a_capped_skirmish_states_its_terms() {
        let mut scenario = Scenario::skirmish();
        for p in scenario.players.iter_mut() {
            p.bot = true;
        }
        let m = sample_match(&scenario, 400, 20).expect("samples");
        assert_eq!(m.ticks, 400);
        assert!(m.capped && m.result.is_none() && m.winners.is_empty());
        assert!(m.last_progress_tick > 0 && m.last_progress_tick <= m.ticks);
        assert_eq!(m.factions, vec!["ferrous", "cupric"]);
        assert_eq!(m.pace, "standard");
        for seat in &m.buildings {
            assert_eq!(seat.get("foundry"), Some(&1), "each seat holds a Foundry");
        }
        assert_eq!(m.entropy_bits.len(), 2);
        assert_eq!(m.count_entropy_bits.len(), 2);
        assert_eq!(m.combat_entropy_bits.len(), 2);
        assert_eq!(m.combat_count_entropy_bits.len(), 2);
    }

    #[test]
    fn entropy_zeroes_on_a_one_kind_army_and_grows_with_spread() {
        let one = record(vec![mix(&[("sentinel", 1.0)])]);
        assert!(aggregate(&[one]).entropy_bits.abs() < 1e-9);
        let spread = record(vec![mix(&[
            ("sentinel", 0.25),
            ("scuttler", 0.25),
            ("lancer", 0.25),
            ("bombard", 0.25),
        ])]);
        let agg = aggregate(&[spread]);
        assert!(
            (agg.entropy_bits - 2.0).abs() < 1e-9,
            "even four-way mix is 2 bits"
        );
    }

    /// The finding the mean cannot report: two seats spamming different
    /// kinds average to a mix that looks diverse, and only the per-seat
    /// floor says otherwise.
    #[test]
    fn two_spamming_seats_average_to_a_diverse_looking_mix() {
        let m = record(vec![mix(&[("sentinel", 1.0)]), mix(&[("scuttler", 1.0)])]);
        let agg = aggregate(&[m]);
        assert!(
            (agg.entropy_bits - 1.0).abs() < 1e-9,
            "the mean of two spams reads as a one-bit mix"
        );
        let spread = agg.seat_entropy.expect("two seats fielded units");
        assert_eq!((spread.mean, spread.p10, spread.median), (0.0, 0.0, 0.0));
    }

    /// Seats that never fielded a unit are not seats; a match all of
    /// whose seats are empty contributes nothing at all.
    #[test]
    fn empty_seats_never_reach_the_fold() {
        let agg = aggregate(&[record(vec![BTreeMap::new(), BTreeMap::new()])]);
        assert_eq!((agg.seats, agg.matches, agg.capped), (0, 0, 0));
        assert!(agg.seat_entropy.is_none() && agg.mean_share.is_empty());
        assert!(
            agg.seat_count_entropy.is_none()
                && agg.seat_count_dominance.is_none()
                && agg.mean_count_share.is_empty()
        );
        assert!(
            agg.seat_combat_entropy.is_none()
                && agg.seat_combat_count_entropy.is_none()
                && agg.seat_combat_count_dominance.is_none()
                && agg.mean_combat_share.is_empty()
                && agg.mean_combat_count_share.is_empty()
                && agg.combat_seats == 0
        );
    }

    /// Cohorts are seat-level: one match splits across two faction
    /// cohorts and is counted once in each.
    #[test]
    fn faction_cohorts_split_one_match_by_seat() {
        let m = record(vec![
            mix(&[("sentinel", 1.0)]),
            mix(&[("scuttler", 0.5), ("darter", 0.5)]),
        ]);
        let by = aggregate_by(&[m], by_faction);
        assert_eq!(by.len(), 2);
        let ferrous = &by["ferrous"];
        assert_eq!((ferrous.seats, ferrous.matches, ferrous.capped), (1, 1, 1));
        assert_eq!(ferrous.mean_share["sentinel"], 1.0);
        assert!(by["cupric"].entropy_bits > 0.9);
    }

    /// The outcome cohort is what the fun gate reads: a stalemate's mix
    /// is evidence about a stalemate.
    #[test]
    fn the_outcome_cohort_separates_decided_from_capped() {
        let mut decided = record(vec![mix(&[("sentinel", 1.0)])]);
        decided.capped = false;
        decided.result = Some(GameResult::Victory { team: 0 });
        decided.winners = vec![0];
        let capped = record(vec![mix(&[("harvester", 1.0)])]);
        let by = aggregate_by(&[decided, capped], by_outcome);
        assert_eq!(by["decided"].matches, 1);
        assert_eq!(by["decided"].decided, 1);
        assert_eq!(by["decided"].capped, 0);
        assert!(by["decided"].mean_share.contains_key("sentinel"));
        assert_eq!(by["capped"].capped, 1);
        assert!(by["capped"].mean_share.contains_key("harvester"));
    }

    /// Building counts answer two different questions: how many, and
    /// how many seats got there at all.
    #[test]
    fn building_counts_report_both_the_mean_and_the_reach() {
        let mut m = record(vec![mix(&[("sentinel", 1.0)]), mix(&[("sentinel", 1.0)])]);
        m.buildings[0] = [("foundry".to_string(), 1), ("fabricator".to_string(), 3)]
            .into_iter()
            .collect();
        m.buildings[1] = [("foundry".to_string(), 1)].into_iter().collect();
        m.competitive_buildings = m.buildings.clone();
        let agg = aggregate(&[m]);
        assert_eq!(agg.mean_buildings["foundry"], 1.0);
        assert_eq!(agg.mean_buildings["fabricator"], 1.5);
        assert_eq!(agg.seats_with_building["foundry"], 1.0);
        assert_eq!(agg.seats_with_building["fabricator"], 0.5);
        assert_eq!(agg.competitive_mean_buildings["fabricator"], 1.5);
        assert_eq!(agg.competitive_seats_with_building["fabricator"], 0.5);
    }

    #[test]
    fn body_count_exposes_cheap_unit_spam_hidden_by_value() {
        let mut m = record(vec![mix(&[("scuttler", 0.4), ("bombard", 0.6)])]);
        m.count_seats[0] = mix(&[("scuttler", 0.84), ("bombard", 0.16)]);
        m.count_entropy_bits[0] = entropy(m.count_seats[0].values());
        let agg = aggregate(&[m]);
        assert_eq!(agg.mean_share["scuttler"], 0.4);
        assert_eq!(agg.mean_count_share["scuttler"], 0.84);
        assert!(agg.count_entropy_bits < agg.entropy_bits);
        let dominance = agg
            .seat_count_dominance
            .expect("one seat supplies a dominance reading");
        assert_eq!(
            (dominance.mean, dominance.p90, dominance.max),
            (0.84, 0.84, 0.84)
        );
    }

    #[test]
    fn harvesters_remain_diagnostic_but_cannot_inflate_combat_diversity() {
        let mut m = record(vec![mix(&[("harvester", 0.5), ("sentinel", 0.5)])]);
        m.combat_seats[0] = mix(&[("sentinel", 1.0)]);
        m.combat_entropy_bits[0] = 0.0;
        m.combat_count_seats[0] = mix(&[("sentinel", 1.0)]);
        m.combat_count_entropy_bits[0] = 0.0;

        let agg = aggregate(&[m]);
        assert_eq!(agg.mean_share["harvester"], 0.5);
        assert!((agg.entropy_bits - 1.0).abs() < 1e-9);
        assert!(!agg.mean_combat_share.contains_key("harvester"));
        assert_eq!(agg.mean_combat_share["sentinel"], 1.0);
        assert_eq!(agg.combat_entropy_bits, 0.0);
        assert_eq!(agg.combat_count_entropy_bits, 0.0);
        assert_eq!(agg.seat_combat_entropy.expect("combat seat").p10, 0.0);
    }

    #[test]
    fn a_resigned_seat_contributes_diagnostics_but_no_competitive_combat() {
        let scenario = Scenario::skirmish();
        let m = sample_driven(&scenario, 1, 1, |state| {
            state.tick(&[oxide_sim::PlayerCommand {
                player: oxide_sim::PlayerId(0),
                command: oxide_sim::Command::Surrender,
            }])
        })
        .expect("samples");

        assert!(
            m.seats[0].contains_key("sentinel"),
            "all-time diagnostics retain resigned units"
        );
        assert!(
            m.combat_seats[0].is_empty(),
            "a resigned seat has left its competitive lifetime"
        );
        assert!(m.final_economy.seats[0].resigned);
        assert!(!m.final_economy.seats[0].recovery_income_active);
        assert!(
            m.combat_seats[1].contains_key("sentinel"),
            "the still-competitive opponent remains evidence"
        );
        let agg = aggregate(&[m]);
        assert_eq!(agg.seats, 2);
        assert_eq!(agg.combat_seats, 1);
    }

    #[test]
    fn a_surrendered_team_seat_keeps_only_its_pre_concession_combat() {
        let scenario = Scenario::from_json(include_str!("../../scenarios/broad-front.json"))
            .expect("the shipped team scenario parses");
        let mut step = 0;
        let m = sample_driven(&scenario, 41, 20, |state| {
            let commands = (step == 21)
                .then_some(oxide_sim::PlayerCommand {
                    player: oxide_sim::PlayerId(0),
                    command: oxide_sim::Command::Surrender,
                })
                .into_iter()
                .collect::<Vec<_>>();
            step += 1;
            state.tick(&commands)
        })
        .expect("samples");

        assert!(m.capped, "the teammate keeps the match alive");
        assert!(m.final_economy.seats[0].resigned);
        assert_eq!(
            m.final_economy.seats[0].living_foundries, 1,
            "resignation, not Foundry destruction, ends this lifetime"
        );
        assert!(
            !m.combat_seats[0].is_empty(),
            "samples before concession remain competitive evidence"
        );
        assert!(
            !m.seats[0].is_empty(),
            "the all-time diagnostic also includes the remnant sample"
        );
    }

    #[test]
    fn seat_filters_apply_to_competitive_combat_metrics() {
        let m = record(vec![
            mix(&[("sentinel", 1.0)]),
            mix(&[("scuttler", 0.5), ("lancer", 0.5)]),
        ]);
        let agg = aggregate_where(&[m], |_, seat| seat == 1);
        assert_eq!((agg.seats, agg.combat_seats), (1, 1));
        assert!(!agg.mean_combat_share.contains_key("sentinel"));
        assert_eq!(agg.mean_combat_share["scuttler"], 0.5);
        assert_eq!(agg.combat_entropy_bits, 1.0);
    }

    #[test]
    fn quantiles_take_the_nearest_rank() {
        assert_eq!(quantile(&[7.0], 1, 10), 7.0);
        assert_eq!(quantile(&[1.0, 2.0, 3.0, 4.0], 1, 2), 2.0);
        assert_eq!(quantile(&[0.0, 1.0, 2.0, 3.0, 4.0], 1, 10), 0.0);
        assert_eq!(
            quantile(&[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0], 9, 10),
            8.0
        );
    }
}
