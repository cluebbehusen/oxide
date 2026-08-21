//! The unit-viability probe: a forced-doctrine A/B that separates
//! "this kind is not worth its cost" from "the policy never learned
//! to reach for it".
//!
//! Two copies of the same quantized policy play the same profile on
//! the same seeds; one carries a doctrine that overrides its chosen
//! head with the probed train/build action whenever that action is
//! mask-legal and the seat holds fewer than the quota of the kind.
//! The doctrine changes nothing else: legality still comes from the
//! shared mask, lowering still goes through the gym, and every other
//! head plays the policy's own game.
//!
//! Determinism makes the comparison exact rather than statistical: a
//! policy mirror is seat-decided per seed, so the probe first plays
//! every seed free-vs-free and then scores each forced game by
//! whether it FLIPPED that baseline. A kind whose doctrine never
//! flips a game is costless to carry — so a free policy that never
//! touches it has a training gap, not a balance excuse. A doctrine
//! that surrenders baseline wins is paying real cost for compliance,
//! which is balance evidence no amount of training would refute.
//!
//! This is QA instrumentation, not a shipped bot: the doctrine seat
//! reads only its own fog-honest observation, and both seats remain
//! ordinary command sources under the level-playing-field contract.

use anyhow::{Context, Result};
use oxide_sim::bot::{
    ACTION_COUNT, Action, ActionPlan, CONSTRUCTION_ACTIONS, GymBot, Level, Observation,
    PRODUCTION_ACTIONS, QuantNet, ladder_condition_values,
};
use oxide_sim::stats::{BuildingKind, Role, UnitKind};
use oxide_sim::{Faction, PlayerId};

/// Probe conditioning: one fixed raw profile for both seats so the
/// only difference between them is the doctrine. Balanced-band
/// aggression with the ladder's full skill and no hesitation.
const PROBE_AGGRESSION: u32 = 550;

/// Every action the default sweep probes, under its CLI name.
pub const PROBE_ACTIONS: &[(&str, Action)] = &[
    ("sentinel", Action::TrainSentinel),
    ("scuttler", Action::TrainScuttler),
    ("lancer", Action::TrainLancer),
    ("bombard", Action::TrainBombard),
    ("anti-air", Action::TrainAntiAir),
    ("air-ground", Action::TrainAirGround),
    ("air-air", Action::TrainAirAir),
    ("warden", Action::TrainWarden),
    ("tender", Action::TrainTender),
    ("excavator", Action::TrainExcavator),
    ("scout-flyer", Action::TrainScoutFlyer),
    ("interceptor", Action::TrainInterceptor),
    ("bomber", Action::TrainBomber),
    ("skyhook", Action::TrainTransport),
    ("sapper", Action::TrainSapper),
    ("breaker", Action::TrainBreaker),
    ("avalanche", Action::TrainAvalanche),
    ("fabricator", Action::BuildFabricator),
    ("turret", Action::BuildTurret),
    ("flak", Action::BuildFlak),
    ("bastion", Action::BuildBastion),
    ("array", Action::BuildArray),
    ("reclaimer", Action::BuildReclaimer),
    ("repair-bay", Action::BuildRepairBay),
    ("airworks", Action::BuildAirworks),
    ("crucible", Action::BuildCrucible),
    ("foundry", Action::BuildFoundry),
    ("extractor", Action::BuildExtractor),
];

/// How many of the probed kind the seat currently expresses: live
/// units of the (faction-resolved) kind, or standing-plus-building
/// structures. Queued units are invisible here, so a doctrine can
/// stack a queue while the first body walks out; the mask's queue
/// ceiling bounds that, and "keep pressure toward K alive" is the
/// quota's intended reading.
fn expressed_count(action: Action, obs: &Observation) -> u32 {
    if let Some(kind) = probed_unit(action, obs.faction) {
        return obs.my_units.iter().filter(|u| u.kind == kind).count() as u32;
    }
    if let Some(kind) = probed_building(action) {
        return obs.my_buildings.iter().filter(|b| b.kind == kind).count() as u32;
    }
    0
}

/// The concrete unit a train action produces for `faction`, when the
/// action trains at all.
fn probed_unit(action: Action, faction: Faction) -> Option<UnitKind> {
    Some(match action {
        Action::TrainSentinel => UnitKind::Sentinel,
        Action::TrainScuttler => UnitKind::Scuttler,
        Action::TrainLancer => UnitKind::Lancer,
        Action::TrainBombard => UnitKind::Bombard,
        Action::TrainAntiAir => Role::AntiAir.unit_for(faction),
        Action::TrainAirGround => Role::AirGround.unit_for(faction),
        Action::TrainAirAir => Role::AirAir.unit_for(faction),
        Action::TrainWarden => UnitKind::Warden,
        Action::TrainTender => UnitKind::Tender,
        Action::TrainExcavator => UnitKind::Excavator,
        Action::TrainScoutFlyer => Role::Scout.unit_for(faction),
        Action::TrainInterceptor => Role::Interceptor.unit_for(faction),
        Action::TrainBomber => Role::Bomber.unit_for(faction),
        Action::TrainTransport => UnitKind::Skyhook,
        Action::TrainSapper => UnitKind::Sapper,
        Action::TrainBreaker => UnitKind::Breaker,
        Action::TrainAvalanche => UnitKind::Avalanche,
        _ => return None,
    })
}

/// The structure a build action starts, when the action builds at all.
fn probed_building(action: Action) -> Option<BuildingKind> {
    Some(match action {
        Action::BuildFabricator => BuildingKind::Fabricator,
        Action::BuildTurret => BuildingKind::Turret,
        Action::BuildFlak => BuildingKind::FlakTurret,
        Action::BuildBastion => BuildingKind::Bastion,
        Action::BuildArray => BuildingKind::Array,
        Action::BuildReclaimer => BuildingKind::Reclaimer,
        Action::BuildRepairBay => BuildingKind::RepairBay,
        Action::BuildAirworks => BuildingKind::Airworks,
        Action::BuildCrucible => BuildingKind::Crucible,
        Action::BuildFoundry => BuildingKind::Foundry,
        Action::BuildExtractor => BuildingKind::Extractor,
        _ => return None,
    })
}

/// The scrap price of committing to `action` this instant: the unit's
/// training cost or the structure's placement cost. Non-probe actions
/// price at zero (the surplus rule never gates them).
fn action_price(action: Action, faction: Faction) -> u32 {
    if let Some(kind) = probed_unit(action, faction) {
        return kind.stats().cost;
    }
    if let Some(kind) = probed_building(action) {
        return kind.base_stats().construction.map(|c| c.cost).unwrap_or(0);
    }
    0
}

/// Producer buildings a train action needs before its mask can open,
/// root-first, so a doctrine below quota can chain toward the probed
/// kind instead of stalling on "not legal yet". Foundry-trained kinds
/// need no chain: every seat starts with its Foundry.
fn producer_chain(action: Action) -> &'static [Action] {
    match action {
        Action::TrainLancer
        | Action::TrainBombard
        | Action::TrainAntiAir
        | Action::TrainWarden
        | Action::TrainTender
        | Action::TrainSapper
        | Action::TrainExcavator => &[Action::BuildFabricator],
        Action::TrainAirGround
        | Action::TrainAirAir
        | Action::TrainScoutFlyer
        | Action::TrainInterceptor
        | Action::TrainTransport => &[Action::BuildAirworks],
        Action::TrainBomber => &[Action::BuildAirworks, Action::BuildCrucible],
        Action::TrainBreaker | Action::TrainAvalanche => &[Action::BuildCrucible],
        _ => &[],
    }
}

/// Per-seat doctrine tallies for one game.
#[derive(Default, Clone, Copy)]
struct DoctrineTally {
    /// Thinks where the doctrine overrode the policy's own choice.
    forced: u64,
    /// Thinks whose final plan carried the probed action (voluntary
    /// picks included) — the census of expressed usage.
    chosen: u64,
    /// Thinks where the quota was unmet but neither the action nor a
    /// chain step was legal: the gate is upstream (queue, scrap, or
    /// terrain), which reads as "could not express" in the report.
    blocked: u64,
}

/// One policy seat, optionally carrying the forcing doctrine. Every
/// seat also tallies a census of the actions its final plans carried.
struct DoctrineBot {
    gym: GymBot,
    net: QuantNet,
    target: Option<(Action, u32)>,
    start_tick: u64,
    tally: DoctrineTally,
    census: [u64; ACTION_COUNT],
}

impl DoctrineBot {
    fn new(
        player: PlayerId,
        net: QuantNet,
        target: Option<(Action, u32)>,
        start_tick: u64,
    ) -> Self {
        Self {
            gym: GymBot::with_cadence(player, Level::Expert.cadence()),
            net,
            target,
            start_tick,
            tally: DoctrineTally::default(),
            census: [0; ACTION_COUNT],
        }
    }

    fn act(&mut self, state: &oxide_sim::State) -> Vec<oxide_sim::PlayerCommand> {
        if !state.current_tick().is_multiple_of(self.gym.cadence()) {
            return Vec::new();
        }
        let decision = self.gym.decision(state);
        let obs = Observation::fog_honest(state, self.gym.player());
        let knobs = ladder_condition_values(PROBE_AGGRESSION, obs.faction);
        let mut plan = self.net.act(&decision, &knobs);
        if let Some((action, quota)) = self.target {
            let index = action as usize;
            // Surplus rule: the doctrine measures INTEGRATING the kind,
            // not rushing it. Forcing an unaffordable choice pins the
            // head on "save up" and starves everything else — a probe
            // of production paralysis, not of the kind. Only a bank
            // that covers the price may be committed to it.
            let affordable = |a: Action| obs.scrap >= action_price(a, obs.faction);
            if state.current_tick() >= self.start_tick && expressed_count(action, &obs) < quota {
                if decision.mask[index] && PRODUCTION_ACTIONS.contains(&index) {
                    if affordable(action) {
                        self.tally.forced += u64::from(plan.production != action);
                        plan.production = action;
                    }
                } else if decision.mask[index] && CONSTRUCTION_ACTIONS.contains(&index) {
                    if affordable(action) {
                        self.tally.forced += u64::from(plan.construction != action);
                        plan.construction = action;
                    }
                } else {
                    // The probed action is closed; chain toward its
                    // missing producer if one can start now, on the
                    // same surplus rule.
                    let step = producer_chain(action).iter().copied().find(|link| {
                        expressed_count(*link, &obs) == 0
                            && decision.mask[*link as usize]
                            && affordable(*link)
                    });
                    if let Some(link) = step {
                        self.tally.forced += u64::from(plan.construction != link);
                        plan.construction = link;
                    } else {
                        self.tally.blocked += 1;
                    }
                }
            }
            self.tally.chosen += u64::from(chosen_in(&plan, action));
        }
        self.census[plan.production as usize] += 1;
        self.census[plan.construction as usize] += 1;
        self.gym.step_plan(state, plan)
    }
}

fn chosen_in(plan: &ActionPlan, action: Action) -> bool {
    plan.production == action || plan.construction == action
}

/// One free-vs-free game: the deterministic yardstick every forced
/// game on the same seed is scored against.
struct BaselineRow {
    winners: Vec<u8>,
    capped: bool,
    census: [u64; ACTION_COUNT],
}

/// One forced game's outcome, from the forced seat's chair.
struct ForcedRow {
    won: bool,
    capped: bool,
    ticks: u64,
    tally: DoctrineTally,
}

fn play_baseline(net: &QuantNet, scenario_name: &str, seed: u64) -> Result<BaselineRow> {
    let mut scenario = crate::runner::load_scenario(scenario_name)?;
    scenario.seed = seed;
    let mut west = DoctrineBot::new(PlayerId(0), net.clone(), None, 0);
    let mut east = DoctrineBot::new(PlayerId(1), net.clone(), None, 0);
    let sampled = oxide_kit::composition::sample_driven(&scenario, BASELINE_TICKS, 20, |state| {
        let mut commands = west.act(state);
        commands.extend(east.act(state));
        state.tick(&commands)
    })
    .context("sampling baseline match")?;
    let mut census = west.census;
    for (total, east_count) in census.iter_mut().zip(east.census) {
        *total += east_count;
    }
    Ok(BaselineRow {
        winners: sampled.winners,
        capped: sampled.capped,
        census,
    })
}

/// Baseline games share the forced games' default cap so a flip is
/// never an artifact of unequal horizons.
const BASELINE_TICKS: u64 = 40_000;

#[allow(clippy::too_many_arguments)]
fn play_forced(
    net: &QuantNet,
    scenario_name: &str,
    seed: u64,
    forced_seat: u8,
    action: Action,
    quota: u32,
    start_tick: u64,
    max_ticks: u64,
) -> Result<ForcedRow> {
    let mut scenario = crate::runner::load_scenario(scenario_name)?;
    scenario.seed = seed;
    let mut forced = DoctrineBot::new(
        PlayerId(forced_seat),
        net.clone(),
        Some((action, quota)),
        start_tick,
    );
    let mut free = DoctrineBot::new(PlayerId(1 - forced_seat), net.clone(), None, 0);
    let sampled = oxide_kit::composition::sample_driven(&scenario, max_ticks, 20, |state| {
        let mut commands = forced.act(state);
        commands.extend(free.act(state));
        state.tick(&commands)
    })
    .context("sampling viability match")?;
    Ok(ForcedRow {
        won: sampled.winners.contains(&forced_seat),
        capped: sampled.capped,
        ticks: sampled.ticks,
        tally: forced.tally,
    })
}

fn run_across_threads<T: Send>(
    jobs: &[(u64, u8)],
    play: impl Fn(&(u64, u8)) -> Result<T> + Sync,
) -> Vec<Result<T>> {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(jobs.len().max(1));
    let chunk = jobs.len().div_ceil(threads);
    let mut rows: Vec<Result<T>> = Vec::with_capacity(jobs.len());
    std::thread::scope(|scope| {
        let play = &play;
        let handles: Vec<_> = jobs
            .chunks(chunk)
            .map(|slice| scope.spawn(move || slice.iter().map(play).collect::<Vec<_>>()))
            .collect();
        for handle in handles {
            rows.extend(handle.join().expect("viability game thread panicked"));
        }
    });
    rows
}

/// Runs the sweep and prints one JSON line per probed action with the
/// flip verdict. `action_filter` narrows to one CLI name.
#[allow(clippy::too_many_arguments)]
pub fn viability_probe(
    weights: &std::path::Path,
    seeds: u64,
    max_ticks: u64,
    scenario: &str,
    quota: u32,
    start_tick: u64,
    action_filter: Option<&str>,
    out: Option<&std::path::Path>,
) -> Result<()> {
    let json = std::fs::read_to_string(weights)
        .with_context(|| format!("reading {}", weights.display()))?;
    let net = QuantNet::from_json(&json).map_err(|e| anyhow::anyhow!(e))?;
    let specs: Vec<(&str, Action)> = match action_filter {
        Some(name) => {
            let spec = PROBE_ACTIONS
                .iter()
                .find(|(candidate, _)| *candidate == name)
                .with_context(|| {
                    let names: Vec<&str> = PROBE_ACTIONS.iter().map(|(n, _)| *n).collect();
                    format!("unknown action {name:?}; expected one of {names:?}")
                })?;
            vec![*spec]
        }
        None => PROBE_ACTIONS.to_vec(),
    };
    eprintln!(
        "viability probe: {} · digest {:016x} · quota {quota} · {seeds} seeds x both seats",
        weights.display(),
        net.digest()
    );

    let seed_jobs: Vec<(u64, u8)> = (3000..3000 + seeds).map(|seed| (seed, 0u8)).collect();
    let baselines: Vec<BaselineRow> =
        run_across_threads(&seed_jobs, |&(seed, _)| play_baseline(&net, scenario, seed))
            .into_iter()
            .collect::<Result<_>>()?;
    let decisive = baselines.iter().filter(|b| !b.winners.is_empty()).count();
    let west_wins = baselines.iter().filter(|b| b.winners.contains(&0)).count();
    // The seat split separates unfairness from mirror chaos: a fair
    // knife-edge map varies its winner by seed; a fixed-seat sweep is
    // a geometry bug no amount of symmetry checking would catch.
    eprintln!(
        "baseline: {decisive}/{seeds} seeds decisive ({west_wins} west), {} capped",
        baselines.iter().filter(|b| b.capped).count()
    );

    let mut report = Vec::new();
    for (name, action) in specs {
        let pairs: Vec<(u64, u8)> = (3000..3000 + seeds)
            .flat_map(|seed| [(seed, 0u8), (seed, 1u8)])
            .collect();
        let rows = run_across_threads(&pairs, |&(seed, seat)| {
            play_forced(
                &net, scenario, seed, seat, action, quota, start_tick, max_ticks,
            )
        });
        let (mut helped, mut hurt, mut stalled, mut unchanged) = (0u64, 0u64, 0u64, 0u64);
        let mut base_won_games = 0u64;
        let mut ticks = Vec::new();
        let (mut forced, mut blocked, mut chosen) = (0u64, 0u64, 0u64);
        let games = pairs.len() as u64;
        for ((seed, seat), row) in pairs.iter().copied().zip(rows) {
            let row = row?;
            let baseline = &baselines[(seed - 3000) as usize];
            let base_won = baseline.winners.contains(&seat);
            base_won_games += u64::from(base_won);
            match (base_won, row.won) {
                (false, true) => helped += 1,
                (true, false) if row.capped && !baseline.capped => stalled += 1,
                (true, false) => hurt += 1,
                _ => unchanged += 1,
            }
            ticks.push(row.ticks);
            forced += row.tally.forced;
            blocked += row.tally.blocked;
            chosen += row.tally.chosen;
        }
        ticks.sort_unstable();
        let expressed = chosen as f64 / games as f64;
        // The census reads voluntary usage off the doctrine-free
        // baseline games (two seats per seed).
        let free_use = baselines
            .iter()
            .map(|b| b.census[action as usize])
            .sum::<u64>() as f64
            / (seeds * 2) as f64;
        let lost_share = (hurt + stalled) as f64 / base_won_games.max(1) as f64;
        // Descriptive bands, not gates. A doctrine that never carried
        // its action proved nothing about the kind — that verdict
        // points at the upstream gate instead.
        let verdict = if expressed < 1.0 {
            "unexpressed"
        } else if hurt + stalled == 0 {
            if helped > 0 {
                "outperforms"
            } else if free_use < 1.0 {
                "costless-unused"
            } else {
                "costless"
            }
        } else if helped >= hurt + stalled {
            "mixed"
        } else if lost_share <= 0.2 {
            "taxed"
        } else {
            "overpriced"
        };
        let row = serde_json::json!({
            "action": name,
            "quota": quota,
            "games": games,
            "helped": helped,
            "hurt": hurt,
            "stalled": stalled,
            "unchanged": unchanged,
            "lost_share": (lost_share * 1000.0).round() / 1000.0,
            "median_ticks": ticks[ticks.len() / 2],
            "forced_overrides_per_game": (forced as f64 / games as f64 * 10.0).round() / 10.0,
            "blocked_per_game": (blocked as f64 / games as f64 * 10.0).round() / 10.0,
            "forced_chosen_per_game": (expressed * 10.0).round() / 10.0,
            "free_chosen_per_seat_game": (free_use * 10.0).round() / 10.0,
            "verdict": verdict,
        });
        println!("{row}");
        report.push(row);
    }
    if let Some(path) = out {
        std::fs::write(path, serde_json::to_string_pretty(&report)?)
            .with_context(|| format!("writing {}", path.display()))?;
        eprintln!("report: {}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_actions_are_unique_and_probeable() {
        let mut names: Vec<&str> = PROBE_ACTIONS.iter().map(|(name, _)| *name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), PROBE_ACTIONS.len(), "duplicate CLI name");
        for (name, action) in PROBE_ACTIONS {
            let index = *action as usize;
            assert!(
                PRODUCTION_ACTIONS.contains(&index) || CONSTRUCTION_ACTIONS.contains(&index),
                "{name} is not a train/build head action"
            );
        }
    }

    #[test]
    fn expressed_count_reads_the_probed_kind() {
        use oxide_sim::Scenario;
        let scenario = crate::runner::load_scenario("skirmish").expect("skirmish loads");
        let state = Scenario::build(&scenario).expect("skirmish builds");
        let obs = Observation::fog_honest(&state, PlayerId(0));
        // Every probed action must resolve to a countable kind — the
        // catch-all zero arm is for non-probe actions only.
        for (name, action) in PROBE_ACTIONS {
            let count = expressed_count(*action, &obs);
            if *action == Action::BuildFoundry {
                assert!(count >= 1, "{name}: the starting Foundry must count");
            }
        }
        assert_eq!(expressed_count(Action::Idle, &obs), 0);
    }
}
