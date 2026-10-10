//! Bot decision cost on fixed workloads.
//!
//! Seats decide serially in seat order with tracing off, so a decision's wall
//! time is the measuring thread's work for that seat. Timing is observational:
//! the commands and the resulting world equal an untimed run of the same
//! scenario, which the report's command and final hashes let a caller check.
//!
//! Beside each decision, the fog-honest observation build is timed by calling
//! it on the same pre-tick world after every seat has decided, so the probe
//! never warms or slows a measured decision.

use anyhow::{Context, Result};
use oxide_kit::GameReplay;
use oxide_kit::controller::{SeatController, record_events, seat_controllers};
use oxide_protocol::hash_hex;
use oxide_sim::observation::ObservationData;
use oxide_sim::scenario::{BotConfig, BotDifficulty, BotStance};
use oxide_sim::{GameResult, PlayerId, Scenario, State};
use serde::Serialize;
use std::time::{Duration, Instant};

/// The profile every bot seat of a named workload plays.
const PROFILE: BotConfig = BotConfig::new(BotDifficulty::Standard, BotStance::Balanced, 0);

/// A named timing workload: one map, its bot seats and their profile, and a
/// tick window from the scenario start. Every bot seat plays Standard,
/// Balanced, personality seed zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workload {
    /// Skirmish Basin at its authored seed with both seats bots, for 6,000
    /// ticks: a representative live duel.
    Duel,
    /// Skyhook Anchorage's seven authored bot seats for 20,000 ticks. The
    /// human first seat stays passive.
    Skyhook,
    /// Basalt Spine terrain, both seats bots, each starting with a mirrored
    /// mature army and tech structures, for 3,000 ticks.
    MatureArmies,
}

impl Workload {
    /// Every named workload.
    pub const ALL: [Self; 3] = [Self::Duel, Self::Skyhook, Self::MatureArmies];

    /// Stable name used on the command line and in reports.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Duel => "duel",
            Self::Skyhook => "skyhook",
            Self::MatureArmies => "mature-armies",
        }
    }

    /// Ticks measured from the scenario start.
    pub const fn ticks(self) -> u64 {
        match self {
            Self::Duel => 6_000,
            Self::Skyhook => 20_000,
            Self::MatureArmies => 3_000,
        }
    }

    /// The workload's scenario with every bot seat configured.
    pub fn scenario(self) -> Scenario {
        let (mut scenario, every_seat) = match self {
            Self::Duel => (Scenario::skirmish(), true),
            Self::Skyhook => (
                embedded(include_str!("../../scenarios/skyhook-anchorage.json")),
                false,
            ),
            Self::MatureArmies => (
                embedded(include_str!(
                    "../tests/fixtures/performance/mature-armies.json"
                )),
                true,
            ),
        };
        for seat in &mut scenario.players {
            seat.bot |= every_seat;
            if seat.bot {
                seat.bot_config = Some(PROFILE);
            }
        }
        scenario
    }
}

fn embedded(json: &str) -> Scenario {
    Scenario::from_json(json).expect("embedded workload scenarios are validated by tests")
}

impl std::fmt::Display for Workload {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.name())
    }
}

impl std::str::FromStr for Workload {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|workload| workload.name() == value)
            .ok_or_else(|| {
                format!("unknown workload `{value}`; expected duel, skyhook, or mature-armies")
            })
    }
}

/// Count, average, nearest-rank p99, maximum and total of nanosecond samples.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Summary {
    /// Samples.
    pub count: u64,
    /// Total divided by count, rounded down.
    pub avg_ns: u64,
    /// The sample at rank `ceil(0.99 * count)` in ascending order.
    pub p99_ns: u64,
    /// Largest sample.
    pub max_ns: u64,
    /// Sum of every sample.
    pub total_ns: u64,
}

impl Summary {
    /// Summarizes `samples` in any order. No samples summarize to zeros.
    pub fn of(samples: &[u64]) -> Self {
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        let count = sorted.len() as u64;
        let total_ns: u64 = sorted.iter().sum();
        let rank = (sorted.len() * 99).div_ceil(100);
        Self {
            count,
            avg_ns: total_ns.checked_div(count).unwrap_or(0),
            p99_ns: rank.checked_sub(1).map_or(0, |index| sorted[index]),
            max_ns: sorted.last().copied().unwrap_or(0),
            total_ns,
        }
    }
}

/// One bot seat's decisions.
#[derive(Debug, Serialize)]
pub struct SeatCost {
    /// Seat index.
    pub player: u8,
    /// Configured difficulty.
    pub difficulty: BotDifficulty,
    /// Configured stance.
    pub stance: BotStance,
    /// Configured personality seed.
    pub personality_seed: u64,
    /// Wall time of each due decision: observation capture plus command
    /// production.
    pub decision: Summary,
    /// `ObservationData::fog_honest` for the seat at each due decision.
    pub observation: Summary,
}

/// Every bot seat, pooled.
#[derive(Debug, Serialize)]
pub struct PooledCost {
    /// Seats pooled.
    pub seats: usize,
    /// Every due decision of those seats.
    pub decision: Summary,
    /// Every observation probe of those seats.
    pub observation: Summary,
}

/// One timed run.
#[derive(Debug, Serialize)]
pub struct CostReport {
    /// Workload name or scenario path.
    pub workload: String,
    /// Scenario display name.
    pub scenario: String,
    /// Requested window in ticks from the scenario start.
    pub ticks: u64,
    /// Tick the run stopped at: the window end or the match result.
    pub final_tick: u64,
    /// Match result, if the run reached one.
    pub result: Option<GameResult>,
    /// Fold of every tick's commands, comparable with an untimed run.
    pub command_hash: String,
    /// Final state hash, comparable with an untimed run.
    pub final_hash: String,
    /// Bot seats in seat order.
    pub seats: Vec<SeatCost>,
    /// Every bot seat pooled; absent without any.
    pub all: Option<PooledCost>,
    /// Wall time of each simulation tick.
    pub simulation: Summary,
}

struct SeatSamples {
    player: PlayerId,
    config: BotConfig,
    decisions: Vec<u64>,
    observations: Vec<u64>,
}

impl SeatSamples {
    fn new(seat: &SeatController, scenario: &Scenario) -> Result<Self> {
        let player = seat.player();
        let config = scenario
            .players
            .get(usize::from(player.0))
            .and_then(|spec| spec.bot_config)
            .context("a seated controller has a configuration")?;
        Ok(Self {
            player,
            config,
            decisions: Vec::new(),
            observations: Vec::new(),
        })
    }

    fn probe(&mut self, state: &State) {
        let start = Instant::now();
        let observation = ObservationData::fog_honest(state, self.player);
        self.observations.push(nanos(start.elapsed()));
        drop(std::hint::black_box(observation));
    }

    fn cost(&self) -> SeatCost {
        SeatCost {
            player: self.player.0,
            difficulty: self.config.difficulty,
            stance: self.config.stance,
            personality_seed: self.config.personality_seed,
            decision: Summary::of(&self.decisions),
            observation: Summary::of(&self.observations),
        }
    }
}

fn nanos(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
}

/// Runs `scenario` from its start for up to `ticks` ticks, stopping early at
/// a match result, and times every bot seat's due decisions. When `replay` is
/// given, the run's commands are recorded into it outside the timed spans.
pub fn measure(
    workload: &str,
    scenario: &Scenario,
    ticks: u64,
    mut replay: Option<&mut GameReplay>,
) -> Result<CostReport> {
    let mut state = scenario.build().context("building scenario")?;
    let mut seats = seat_controllers(scenario).context("building public bot map briefing")?;
    let mut samples = seats
        .iter()
        .map(|seat| SeatSamples::new(seat, scenario))
        .collect::<Result<Vec<_>>>()?;
    let mut due = vec![false; seats.len()];
    let mut command_hash = 0_u64;
    let mut simulation = Vec::new();
    while state.current_tick() < ticks && state.result().is_none() {
        let tick = state.current_tick();
        let mut commands = Vec::new();
        for ((seat, samples), due) in seats.iter_mut().zip(&mut samples).zip(&mut due) {
            *due = seat.decision_due(&state);
            let start = Instant::now();
            let decided = seat.act(&state);
            let elapsed = start.elapsed();
            if *due {
                samples.decisions.push(nanos(elapsed));
            }
            commands.extend(decided);
        }
        for (samples, _) in samples.iter_mut().zip(&due).filter(|(_, due)| **due) {
            samples.probe(&state);
        }
        command_hash = chassis::hash::state_hash(&(command_hash, tick, &commands));
        if let Some(replay) = replay.as_deref_mut() {
            for command in &commands {
                replay.record(tick, command.clone());
            }
        }
        let start = Instant::now();
        let report = state.tick(&commands);
        simulation.push(nanos(start.elapsed()));
        record_events(&mut seats, &report);
    }
    if let Some(replay) = replay {
        replay.meta.ticks = Some(state.current_tick());
    }
    let pooled = |pick: fn(&SeatSamples) -> &[u64]| {
        Summary::of(&samples.iter().flat_map(pick).copied().collect::<Vec<_>>())
    };
    let all = (!samples.is_empty()).then(|| PooledCost {
        seats: samples.len(),
        decision: pooled(|seat| &seat.decisions),
        observation: pooled(|seat| &seat.observations),
    });
    Ok(CostReport {
        workload: workload.to_owned(),
        scenario: scenario.name.clone(),
        ticks,
        final_tick: state.current_tick(),
        result: state.result(),
        command_hash: hash_hex(command_hash),
        final_hash: hash_hex(state.hash()),
        seats: samples.iter().map(SeatSamples::cost).collect(),
        all,
        simulation: Summary::of(&simulation),
    })
}

impl CostReport {
    /// A compact human-readable table: microseconds per decision and
    /// milliseconds in total.
    pub fn table(&self) -> String {
        use std::fmt::Write;
        let micros = |ns: u64| format!("{:.1}", ns as f64 / 1_000.0);
        let millis = |ns: u64| format!("{:.1}", ns as f64 / 1_000_000.0);
        let row = |seat: &str, profile: &str, decision: Summary, observation: Summary| {
            format!(
                "{seat:<5} {profile:<22} {:>9} {:>9} {:>9} {:>9} {:>8} {:>8}",
                decision.count,
                micros(decision.avg_ns),
                micros(decision.p99_ns),
                millis(decision.total_ns),
                micros(observation.avg_ns),
                micros(observation.p99_ns),
            )
        };
        let outcome = match self.result {
            None => "undecided".to_owned(),
            Some(GameResult::Victory { team }) => format!("team {team} won"),
            Some(GameResult::Draw) => "draw".to_owned(),
        };
        let mut out = String::new();
        let _ = writeln!(
            out,
            "{}: {}, ticks 0..{}, stopped at {} ({outcome})",
            self.workload, self.scenario, self.ticks, self.final_tick
        );
        let _ = writeln!(
            out,
            "command hash {}, final hash {}",
            self.command_hash, self.final_hash
        );
        let _ = writeln!(
            out,
            "\n{:<5} {:<22} {:>9} {:>9} {:>9} {:>9} {:>8} {:>8}",
            "seat", "profile", "decisions", "avg µs", "p99 µs", "total ms", "obs avg", "obs p99"
        );
        for seat in &self.seats {
            let profile = format!(
                "{}/{}/{}",
                seat.difficulty, seat.stance, seat.personality_seed
            );
            let _ = writeln!(
                out,
                "{}",
                row(
                    &seat.player.to_string(),
                    &profile,
                    seat.decision,
                    seat.observation,
                )
            );
        }
        if let Some(all) = &self.all {
            let _ = writeln!(
                out,
                "{}",
                row(
                    "all",
                    &format!("{} seats", all.seats),
                    all.decision,
                    all.observation,
                )
            );
        }
        let bots: u64 = self.seats.iter().map(|seat| seat.decision.total_ns).sum();
        let ratio = if self.simulation.total_ns == 0 {
            "n/a".to_owned()
        } else {
            format!("{:.2}", bots as f64 / self.simulation.total_ns as f64)
        };
        let _ = writeln!(
            out,
            "\nsimulation: {} ticks, avg {} µs, p99 {} µs, total {} ms; bots/simulation {ratio}",
            self.simulation.count,
            micros(self.simulation.avg_ns),
            micros(self.simulation.p99_ns),
            millis(self.simulation.total_ns),
        );
        out
    }
}

#[cfg(test)]
mod tests;
