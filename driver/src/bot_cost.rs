//! Bot decision cost on fixed workloads.
//!
//! Seats decide serially in seat order with tracing off, so a decision's wall
//! time is the measuring thread's work for that seat. Timing is observational:
//! the commands and the resulting world equal an untimed run of the same
//! scenario, which the report's command and final hashes let a caller check.
//!
//! Beside each decision, the fog-honest observation build and `oxide-bot`'s
//! orientation are timed by calling their public functions on the same
//! pre-tick world after every seat has decided, so these probes never warm or
//! slow a measured decision.

use anyhow::{Context, Result};
use oxide_bot::{Observation, Orientation};
use oxide_kit::controller::{SeatController, seat_controllers};
use oxide_protocol::hash_hex;
use oxide_sim::observation::ObservationData;
use oxide_sim::scenario::{BotConfig, BotController, BotDifficulty, BotStance};
use oxide_sim::{BuildingKind, GameResult, PlayerId, Scenario, State};
use serde::Serialize;
use std::time::{Duration, Instant};

/// The profile every bot seat of a named workload plays, apart from its
/// controller.
const PROFILE: BotConfig = BotConfig::scripted(BotDifficulty::Standard, BotStance::Balanced, 0);

/// A named timing workload: one map, its bot seats and their profile, and a
/// tick window from the scenario start. Every bot seat plays Standard,
/// Balanced, personality seed zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workload {
    /// Skirmish Basin at its authored seed with both seats bots, for 6,000
    /// ticks: a representative live duel.
    Duel,
    /// Skyhook Anchorage's seven authored bot seats for 20,000 ticks. The
    /// first seat stays passive, as in the shipped performance workload.
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

    /// The workload's scenario with `controller` in every bot seat.
    pub fn scenario(self, controller: BotController) -> Scenario {
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
                seat.bot_config = Some(BotConfig {
                    controller,
                    ..PROFILE
                });
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
    /// Controller driving the seat.
    pub controller: BotController,
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
    /// `oxide-bot`'s `Orientation::observe` of that observation, at due
    /// decisions where the seat has a home Foundry. Absent for other
    /// controllers.
    pub orientation: Option<Summary>,
}

/// Every seat of one controller kind, pooled.
#[derive(Debug, Serialize)]
pub struct ControllerCost {
    /// The controller.
    pub controller: BotController,
    /// Seats it drives.
    pub seats: usize,
    /// Every due decision of those seats.
    pub decision: Summary,
    /// Every observation probe of those seats.
    pub observation: Summary,
    /// Every orientation probe, for `oxide-bot`.
    pub orientation: Option<Summary>,
}

/// One timed run.
#[derive(Debug, Serialize)]
pub struct CostReport {
    /// Workload name or scenario path.
    pub workload: String,
    /// Scenario display name.
    pub scenario: String,
    /// Simulation seed.
    pub seed: u64,
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
    /// Seats pooled by controller, in controller order.
    pub controllers: Vec<ControllerCost>,
    /// Wall time of each simulation tick.
    pub simulation: Summary,
}

struct SeatSamples {
    player: PlayerId,
    config: BotConfig,
    decisions: Vec<u64>,
    observations: Vec<u64>,
    orientations: Vec<u64>,
    /// Latched at the first probe with a home Foundry, as `oxide-bot`
    /// latches its own frame at its first decision.
    frame: Option<Orientation>,
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
            orientations: Vec::new(),
            frame: None,
        })
    }

    fn probe(&mut self, state: &State) {
        let start = Instant::now();
        let observation = ObservationData::fog_honest(state, self.player);
        self.observations.push(nanos(start.elapsed()));
        if self.config.controller != BotController::Scripted {
            return;
        }
        let observation = Observation::from_data(observation);
        let Some(home) = observation
            .my_buildings
            .iter()
            .filter(|building| !building.provisional && building.kind == BuildingKind::Foundry)
            .min_by_key(|building| building.id)
            .map(|building| building.anchor)
        else {
            return;
        };
        let frame = *self
            .frame
            .get_or_insert_with(|| Orientation::for_home(&observation, home));
        let start = Instant::now();
        let oriented = std::hint::black_box(frame.observe(&observation));
        self.orientations.push(nanos(start.elapsed()));
        drop(oriented);
    }

    fn cost(&self) -> SeatCost {
        let scripted = self.config.controller == BotController::Scripted;
        SeatCost {
            player: self.player.0,
            controller: self.config.controller,
            difficulty: self.config.difficulty,
            stance: self.config.stance,
            personality_seed: self.config.personality_seed,
            decision: Summary::of(&self.decisions),
            observation: Summary::of(&self.observations),
            orientation: scripted.then(|| Summary::of(&self.orientations)),
        }
    }
}

fn nanos(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
}

/// Runs `scenario` from its start for up to `ticks` ticks, stopping early at
/// a match result, and times every bot seat's due decisions.
pub fn measure(workload: &str, scenario: &Scenario, ticks: u64) -> Result<CostReport> {
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
        let start = Instant::now();
        state.tick(&commands);
        simulation.push(nanos(start.elapsed()));
    }
    let controllers = BotController::ALL
        .into_iter()
        .filter_map(|controller| {
            let seats: Vec<_> = samples
                .iter()
                .filter(|seat| seat.config.controller == controller)
                .collect();
            let pooled = |pick: fn(&SeatSamples) -> &[u64]| {
                Summary::of(
                    &seats
                        .iter()
                        .flat_map(|seat| pick(seat))
                        .copied()
                        .collect::<Vec<_>>(),
                )
            };
            (!seats.is_empty()).then(|| ControllerCost {
                controller,
                seats: seats.len(),
                decision: pooled(|seat| &seat.decisions),
                observation: pooled(|seat| &seat.observations),
                orientation: (controller == BotController::Scripted)
                    .then(|| pooled(|seat| &seat.orientations)),
            })
        })
        .collect();
    Ok(CostReport {
        workload: workload.to_owned(),
        scenario: scenario.name.clone(),
        seed: scenario.seed,
        ticks,
        final_tick: state.current_tick(),
        result: state.result(),
        command_hash: hash_hex(command_hash),
        final_hash: hash_hex(state.hash()),
        seats: samples.iter().map(SeatSamples::cost).collect(),
        controllers,
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
        let row = |seat: &str,
                   controller: BotController,
                   profile: &str,
                   decision: Summary,
                   observation: Summary,
                   orientation: Option<Summary>| {
            let (orient_avg, orient_p99) = orientation.map_or_else(
                || ("-".to_owned(), "-".to_owned()),
                |summary| (micros(summary.avg_ns), micros(summary.p99_ns)),
            );
            format!(
                "{seat:<5} {:<10} {profile:<22} {:>9} {:>9} {:>9} {:>9} {:>8} {:>8} {:>10} {:>10}",
                controller.as_str(),
                decision.count,
                micros(decision.avg_ns),
                micros(decision.p99_ns),
                millis(decision.total_ns),
                micros(observation.avg_ns),
                micros(observation.p99_ns),
                orient_avg,
                orient_p99,
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
            "{}: {} (seed {}), ticks 0..{}, stopped at {} ({outcome})",
            self.workload, self.scenario, self.seed, self.ticks, self.final_tick
        );
        let _ = writeln!(
            out,
            "command hash {}, final hash {}",
            self.command_hash, self.final_hash
        );
        let _ = writeln!(
            out,
            "\n{:<5} {:<10} {:<22} {:>9} {:>9} {:>9} {:>9} {:>8} {:>8} {:>10} {:>10}",
            "seat",
            "controller",
            "profile",
            "decisions",
            "avg µs",
            "p99 µs",
            "total ms",
            "obs avg",
            "obs p99",
            "orient avg",
            "orient p99"
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
                    seat.controller,
                    &profile,
                    seat.decision,
                    seat.observation,
                    seat.orientation,
                )
            );
        }
        for controller in &self.controllers {
            let _ = writeln!(
                out,
                "{}",
                row(
                    "all",
                    controller.controller,
                    &format!("{} seats", controller.seats),
                    controller.decision,
                    controller.observation,
                    controller.orientation,
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
mod tests {
    use super::*;
    use oxide_sim::scenario::ScenarioMode;

    #[test]
    fn summary_reports_nearest_rank_p99_and_integer_average() {
        assert_eq!(Summary::of(&[]), Summary::default());
        assert_eq!(
            Summary::of(&[7]),
            Summary {
                count: 1,
                avg_ns: 7,
                p99_ns: 7,
                max_ns: 7,
                total_ns: 7
            }
        );
        let mut hundred: Vec<u64> = (1..=100).collect();
        hundred.reverse();
        hundred.swap(3, 71);
        assert_eq!(
            Summary::of(&hundred),
            Summary {
                count: 100,
                avg_ns: 50,
                p99_ns: 99,
                max_ns: 100,
                total_ns: 5050
            }
        );
        let hundred_fifty: Vec<u64> = (1..=150).map(|value| value * 10).collect();
        let summary = Summary::of(&hundred_fifty);
        assert_eq!((summary.p99_ns, summary.max_ns), (1490, 1500));
        assert_eq!(summary.avg_ns, 755);
    }

    #[test]
    fn workloads_name_their_maps_seats_and_windows() {
        let seats: [&[u8]; 3] = [&[0, 1], &[1, 2, 3, 4, 5, 6, 7], &[0, 1]];
        for (workload, expected) in Workload::ALL.into_iter().zip(seats) {
            assert_eq!(workload.name().parse(), Ok(workload));
            assert!(workload.ticks() > 0);
            for controller in BotController::ALL {
                let scenario = workload.scenario(controller);
                scenario.build().expect("workload scenarios build");
                let seated = seat_controllers(&scenario).unwrap();
                assert_eq!(
                    seated
                        .iter()
                        .map(|seat| seat.player().0)
                        .collect::<Vec<_>>(),
                    expected,
                    "{workload} seats"
                );
                for seat in &seated {
                    assert_eq!(seat.controller(), controller);
                    assert_eq!(
                        scenario.players[usize::from(seat.player().0)].bot_config,
                        Some(BotConfig {
                            controller,
                            ..PROFILE
                        })
                    );
                }
            }
        }
        assert_eq!(Workload::Skyhook.ticks(), 20_000);
        assert!("oracle".parse::<Workload>().is_err());
    }

    #[test]
    fn mature_armies_mirror_an_army_and_structures_for_each_seat() {
        let scenario = Workload::MatureArmies.scenario(BotController::Scripted);
        assert_eq!(scenario.mode, ScenarioMode::Match);
        let state = scenario.build().unwrap();
        let holdings = |player: PlayerId| {
            let mut units: Vec<_> = state
                .units()
                .iter()
                .filter(|unit| unit.player == player)
                .map(|unit| unit.kind.role() as u8)
                .collect();
            units.sort();
            let mut structures: Vec<_> = state
                .buildings()
                .iter()
                .filter(|building| building.player == player)
                .map(|building| building.kind)
                .collect();
            structures.sort();
            (units, structures)
        };
        let (units, structures) = holdings(PlayerId(0));
        assert!(units.len() >= 40, "a mature army, not an opening");
        assert!(structures.contains(&BuildingKind::Fabricator));
        assert!(structures.contains(&BuildingKind::Airworks));
        assert_eq!(holdings(PlayerId(1)), (units, structures));
    }
}
