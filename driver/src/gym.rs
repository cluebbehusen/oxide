//! The training loop's other half: episodes over stdio.
//!
//! `oxide-driver gym` speaks newline-delimited JSON on stdin/stdout so
//! a trainer (see `tools/train/`) can drive [`GymBot`] episodes without
//! linking Rust. `control` names the externally-driven seats: one seat
//! against a scripted tier for curriculum and evaluation, or both
//! seats for self-play and league play — every decision tick then
//! carries features and masks for each controlled seat, and `step`
//! takes one production/construction/operation action triple per
//! controlled seat, in the same order.
//! Determinism holds the whole way down: same seed and same actions
//! replay the same match, which is what makes rollouts auditable.

use anyhow::{Context, Result, bail};
use oxide_sim::bot::{
    ACTION_COUNT, ACTION_HEADS, Action, ActionPlan, Brain, Decision, Difficulty, FEATURE_COUNT,
    FEATURE_NAMES, GYM_VERSION, GymBot,
};
use oxide_sim::scenario::Scenario;
use oxide_sim::state::GameResult;
use oxide_sim::{
    BuildingKind, Command, Event, Faction, PlayerCommand, PlayerId, State, UnitRepairSource,
};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};

/// Ordered west/east faction pair for a two-seat neural cup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DuelFactions([Faction; 2]);

impl DuelFactions {
    /// Compact ordered pair used by the CLI and JSON provenance.
    pub fn code(self) -> &'static str {
        match self.0 {
            [Faction::Ferrous, Faction::Ferrous] => "ff",
            [Faction::Ferrous, Faction::Cupric] => "fc",
            [Faction::Cupric, Faction::Ferrous] => "cf",
            [Faction::Cupric, Faction::Cupric] => "cc",
        }
    }

    fn from_scenario(scenario: &Scenario) -> Result<Self> {
        if scenario.players.len() != 2 {
            bail!(
                "neural-cup requires a 2-seat duel scenario, got {} seats",
                scenario.players.len()
            );
        }
        Ok(Self([
            scenario.players[0].faction,
            scenario.players[1].faction,
        ]))
    }

    fn faction(self, seat: u8) -> Faction {
        self.0[usize::from(seat)]
    }
}

impl std::str::FromStr for DuelFactions {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "ff" => Ok(Self([Faction::Ferrous, Faction::Ferrous])),
            "fc" => Ok(Self([Faction::Ferrous, Faction::Cupric])),
            "cf" => Ok(Self([Faction::Cupric, Faction::Ferrous])),
            "cc" => Ok(Self([Faction::Cupric, Faction::Cupric])),
            _ => Err(format!(
                "invalid faction pair {value:?}; expected ff, fc, cf, or cc"
            )),
        }
    }
}

/// Runtime profile for a native quantized neural cup.
#[derive(Debug, Clone, Copy)]
pub struct NeuralCupProfile {
    /// Raw decision-cadence override; `None` uses Expert's cadence.
    pub cadence: Option<u64>,
    /// Tick cap for each tournament game.
    pub max_ticks: u64,
    /// Exact hesitation override. `Some(0)` means no hesitation.
    pub blunder: Option<u32>,
    /// Raw skill-conditioning override.
    pub skill: Option<u32>,
    /// Aggression conditioning knob.
    pub aggression: u32,
    /// Optional ordered roster override.
    pub factions: Option<DuelFactions>,
}

type CupOutcome = (bool, bool, bool, bool, u64);

fn faction_name(faction: Faction) -> &'static str {
    match faction {
        Faction::Ferrous => "ferrous",
        Faction::Cupric => "cupric",
    }
}

fn prepare_cup_scenario(
    scenario: &str,
    seed: u64,
    factions: Option<DuelFactions>,
) -> Result<(Scenario, DuelFactions)> {
    let mut scenario = crate::runner::load_scenario(scenario)?;
    scenario.seed = seed;
    let authored = DuelFactions::from_scenario(&scenario)?;
    let actual = factions.unwrap_or(authored);
    if factions.is_some() {
        for seat in 0..2 {
            scenario.retint_seat(seat, actual.faction(seat as u8));
        }
    }
    Ok((scenario, actual))
}

#[derive(Clone, Copy)]
enum CupOpponent {
    Tier(Difficulty),
    Rusher,
}

impl CupOpponent {
    fn name(self) -> String {
        match self {
            Self::Tier(tier) => format!("{tier:?}"),
            Self::Rusher => "Rusher".to_string(),
        }
    }
}

fn decision_feature(decision: &Decision, name: &str) -> i64 {
    let index = FEATURE_NAMES
        .iter()
        .position(|candidate| *candidate == name)
        .expect("cup teacher names a gym feature");
    decision.features[index]
}

/// The known opening exploit used during training, expressed against
/// the same Rust action contract as the candidate. Keeping it in the
/// native cup means the exact Q12 actor faces the canary rather than a
/// float reconstruction with a different tanh implementation.
fn cup_rusher_plan(decision: &Decision, tick: u64) -> ActionPlan {
    let mut plan = ActionPlan::default();
    let harvesters = decision_feature(decision, "my_harvesters");
    if harvesters < 4 && decision.mask[Action::TrainHarvester as usize] {
        plan.production = Action::TrainHarvester;
    } else if decision.mask[Action::TrainSentinel as usize] {
        plan.production = Action::TrainSentinel;
    }

    let staging = decision_feature(decision, "staging_army_size");
    if decision.mask[Action::Push as usize] && staging >= 5 {
        plan.operation = Action::Push;
    } else if decision.mask[Action::FormArmy as usize] {
        plan.operation = Action::FormArmy;
    } else if decision.mask[Action::Scout as usize] && tick.is_multiple_of(1024) {
        plan.operation = Action::Scout;
    }
    plan
}

#[derive(Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
enum Request {
    Reset {
        seed: u64,
        /// Externally-driven seats (default `[0]`).
        #[serde(default = "default_control")]
        control: Vec<u8>,
        /// Scripted opponent tier for any uncontrolled seat.
        #[serde(default = "default_tier")]
        tier: Difficulty,
        #[serde(default = "default_max_ticks")]
        max_ticks: u64,
        #[serde(default)]
        scenario: Option<String>,
        /// Optional roster for every scenario seat, in seat order.
        /// Omitted resets preserve the scenario's authored factions.
        #[serde(default)]
        factions: Option<Vec<Faction>>,
        /// Decision stride in ticks (default 8).
        #[serde(default = "default_cadence")]
        cadence: u64,
    },
    Step {
        /// One action triple per controlled seat, in `control` order.
        actions: Vec<[usize; 3]>,
    },
    Quit,
}

fn default_control() -> Vec<u8> {
    vec![0]
}

fn default_tier() -> Difficulty {
    Difficulty::Veteran
}

fn default_max_ticks() -> u64 {
    40_000
}

fn default_cadence() -> u64 {
    8
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct SeatEffects {
    seat: u8,
    repair_unit_commands: u64,
    unit_hp_restored: u64,
    repair_unit_hp_restored: u64,
    buildings_salvaged: u64,
    buildings_completed: Vec<BuildingKind>,
}

impl SeatEffects {
    fn new(seat: PlayerId) -> Self {
        Self {
            seat: seat.0,
            repair_unit_commands: 0,
            unit_hp_restored: 0,
            repair_unit_hp_restored: 0,
            buildings_salvaged: 0,
            buildings_completed: Vec::new(),
        }
    }

    fn clear(&mut self) {
        self.repair_unit_commands = 0;
        self.unit_hp_restored = 0;
        self.repair_unit_hp_restored = 0;
        self.buildings_salvaged = 0;
        self.buildings_completed.clear();
    }
}

struct Episode {
    state: State,
    gyms: Vec<GymBot>,
    opponents: Vec<Brain>,
    max_ticks: u64,
    effects: Vec<SeatEffects>,
}

impl Episode {
    fn new(
        seed: u64,
        control: &[u8],
        tier: Difficulty,
        max_ticks: u64,
        scenario: Option<&str>,
        factions: Option<&[Faction]>,
        cadence: u64,
    ) -> Result<Self> {
        let mut scenario = crate::runner::load_scenario(scenario.unwrap_or("skirmish"))?;
        scenario.seed = seed;
        let players = scenario.players.len() as u8;
        if control.is_empty() || control.len() > players as usize {
            bail!("control must name 1..={players} seats");
        }
        let mut seen = control.to_vec();
        seen.sort_unstable();
        seen.dedup();
        if seen.len() != control.len() || control.iter().any(|s| *s >= players) {
            bail!("controlled seats must be distinct and < {players}");
        }
        if let Some(factions) = factions {
            if factions.len() != usize::from(players) {
                bail!(
                    "factions must name exactly {players} seats, got {}",
                    factions.len()
                );
            }
            for (seat, &faction) in factions.iter().enumerate() {
                scenario.retint_seat(seat, faction);
            }
        }
        let state = scenario.build().context("scenario build")?;
        let gyms: Vec<GymBot> = control
            .iter()
            .map(|s| GymBot::with_cadence(PlayerId(*s), cadence))
            .collect();
        let opponents: Vec<Brain> = (0..players)
            .filter(|s| !control.contains(s))
            .map(|s| Brain::for_tier(PlayerId(s), seed, tier))
            .collect();
        let effects = control
            .iter()
            .map(|&seat| SeatEffects::new(PlayerId(seat)))
            .collect();
        Ok(Self {
            state,
            gyms,
            opponents,
            max_ticks,
            effects,
        })
    }

    /// Whether a controlled seat still holds a Foundry. An eliminated
    /// learner's remnants keep fighting (sim rule), but its episode is
    /// over — spectating to the tick cap teaches nothing.
    fn seat_alive(&self, seat: PlayerId) -> bool {
        self.state
            .buildings()
            .iter()
            .any(|b| b.player == seat && b.kind == oxide_sim::BuildingKind::Foundry)
    }

    /// Whether any seat on this seat's *team* still holds a Foundry —
    /// the episode a dead learner belongs to lives while its team does.
    fn team_alive(&self, seat: PlayerId) -> bool {
        let team = self.state.player(seat).team;
        self.state.buildings().iter().any(|b| {
            b.kind == oxide_sim::BuildingKind::Foundry && self.state.player(b.player).team == team
        })
    }

    /// True while the match is live, any controlled seat's *team*
    /// stands, and the tick cap is unmet. A dead learner drops out of
    /// the per-frame `seats` list (its `alive` flag goes false) but its
    /// episode runs on to the team outcome — ending it at the learner's
    /// own death would score every self-sacrifice as a loss, even when
    /// the scripted ally goes on to win (the mixed-ally `team2` role
    /// fields exactly that shape). Per-seat trajectory truncation is
    /// the trainer's bookkeeping.
    fn live(&self) -> bool {
        self.state.result().is_none()
            && self.state.current_tick() < self.max_ticks
            && self.gyms.iter().any(|g| self.team_alive(g.player()))
    }

    /// Whether the tick budget, rather than a game result or learner
    /// elimination, ended this episode. Consumers need this distinction
    /// to bootstrap a living learner's value at the artificial boundary.
    fn truncated(&self) -> bool {
        self.state.result().is_none() && self.state.current_tick() >= self.max_ticks
    }

    fn cadence(&self) -> u64 {
        self.gyms[0].cadence()
    }

    fn note_events(&mut self, events: &[Event]) {
        for event in events {
            match event {
                Event::BuildingCompleted { player, kind, .. } => {
                    if let Some(effect) = self
                        .effects
                        .iter_mut()
                        .find(|effect| effect.seat == player.0)
                    {
                        effect.buildings_completed.push(*kind);
                    }
                }
                Event::BuildingSalvaged { player, .. } => {
                    if let Some(effect) = self
                        .effects
                        .iter_mut()
                        .find(|effect| effect.seat == player.0)
                    {
                        effect.buildings_salvaged += 1;
                    }
                }
                Event::UnitRepaired {
                    player,
                    source,
                    amount,
                    ..
                } => {
                    if let Some(effect) = self
                        .effects
                        .iter_mut()
                        .find(|effect| effect.seat == player.0)
                    {
                        effect.unit_hp_restored += u64::from(*amount);
                        if matches!(source, UnitRepairSource::FieldWelder { .. }) {
                            effect.repair_unit_hp_restored += u64::from(*amount);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn tick_with_effects(&mut self, commands: &[PlayerCommand]) {
        for command in commands {
            let Some(effect) = self
                .effects
                .iter_mut()
                .find(|effect| effect.seat == command.player.0)
            else {
                continue;
            };
            if matches!(&command.command, Command::RepairUnit { .. }) {
                effect.repair_unit_commands += 1;
            }
        }

        let report = self.state.tick(commands);
        self.note_events(&report.events);
    }

    /// Applies the trainer's actions at the current decision tick, then
    /// advances to the next decision tick (or the end).
    fn step(&mut self, actions: &[[usize; 3]]) -> Result<()> {
        // One action triple per *living* controlled seat, in seat order
        // — dead learners dropped out of the frame's seats list and send
        // none.
        let live: Vec<usize> = (0..self.gyms.len())
            .filter(|&i| self.seat_alive(self.gyms[i].player()))
            .collect();
        if actions.len() != live.len() {
            bail!(
                "expected {} action triples (one per living controlled seat), got {}",
                live.len(),
                actions.len()
            );
        }
        for effect in &mut self.effects {
            effect.clear();
        }
        let mut commands = Vec::new();
        for (&idx, action) in live.iter().zip(actions) {
            commands
                .extend(self.gyms[idx].step_plan(&self.state, ActionPlan::from_indices(*action)));
        }
        for op in self.opponents.iter_mut() {
            commands.extend(op.act(&self.state));
        }
        self.tick_with_effects(&commands);
        while self.live() && !self.state.current_tick().is_multiple_of(self.cadence()) {
            let mut commands = Vec::new();
            for op in self.opponents.iter_mut() {
                commands.extend(op.act(&self.state));
            }
            self.tick_with_effects(&commands);
        }
        Ok(())
    }

    fn reply(&mut self) -> serde_json::Value {
        let alive: Vec<u8> = self
            .gyms
            .iter()
            .map(|g| g.player())
            .filter(|p| self.seat_alive(*p))
            .map(|p| p.0)
            .collect();
        let factions: Vec<Faction> = self.state.players().iter().map(|p| p.faction).collect();
        if self.live() {
            let state = &self.state;
            let live_seats: Vec<PlayerId> = self
                .gyms
                .iter()
                .map(|g| g.player())
                .filter(|p| {
                    state
                        .buildings()
                        .iter()
                        .any(|b| b.player == *p && b.kind == oxide_sim::BuildingKind::Foundry)
                })
                .collect();
            let seats: Vec<_> = self
                .gyms
                .iter_mut()
                .filter(|g| live_seats.contains(&g.player()))
                .map(|gym| {
                    let d = gym.decision(state);
                    serde_json::json!({
                        "seat": gym.player().0,
                        "features": d.features.to_vec(),
                        "mask": d.mask.to_vec(),
                    })
                })
                .collect();
            serde_json::json!({
                "done": false,
                "tick": self.state.current_tick(),
                "seats": seats,
                "alive": alive,
                "factions": factions,
                "effects": &self.effects,
            })
        } else {
            let truncated = self.truncated();
            // `winner` is the surviving *team*; in a 1v1 (where every
            // seat defaults to its own team) that is also the seat index.
            let winner = match self.state.result() {
                Some(GameResult::Victory { team }) => Some(team),
                _ => None,
            };
            let winners: Vec<u8> = self.state.winners().into_iter().map(|p| p.0).collect();
            // v5: the terminal frame carries observations for LIVING
            // controlled seats, matching the nonterminal contract — a
            // post-mortem row for a dead seat would zero the frozen
            // last view its padding exists to preserve. The trainer
            // prices terminal potential and the tech bonus off this
            // frame instead of the previous decision's stale one.
            let state = &self.state;
            let live_seats: Vec<PlayerId> = self
                .gyms
                .iter()
                .map(|g| g.player())
                .filter(|p| {
                    state
                        .buildings()
                        .iter()
                        .any(|b| b.player == *p && b.kind == oxide_sim::BuildingKind::Foundry)
                })
                .collect();
            let seats: Vec<_> = self
                .gyms
                .iter_mut()
                .filter(|g| live_seats.contains(&g.player()))
                .map(|gym| {
                    let d = gym.decision(state);
                    serde_json::json!({
                        "seat": gym.player().0,
                        "features": d.features.to_vec(),
                        "mask": d.mask.to_vec(),
                    })
                })
                .collect();
            serde_json::json!({
                "done": true,
                "truncated": truncated,
                "tick": self.state.current_tick(),
                "winner": winner,
                "winners": winners,
                "alive": alive,
                "seats": seats,
                "factions": factions,
                "effects": &self.effects,
            })
        }
    }
}

fn hello() -> serde_json::Value {
    serde_json::json!({
        "ready": true,
        "version": GYM_VERSION,
        "names": oxide_sim::bot::FEATURE_NAMES.to_vec(),
        "features": FEATURE_COUNT,
        "actions": ACTION_COUNT,
        "action_heads": ACTION_HEADS
            .iter()
            .map(|head| head.to_vec())
            .collect::<Vec<_>>(),
        "reset_factions": true,
        "effect_telemetry": true,
    })
}

/// Runs the stdio loop until EOF or a quit command.
pub fn serve() -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(out, "{}", hello())?;
    out.flush()?;

    let mut episode: Option<Episode> = None;
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Request>(&line) {
            Ok(Request::Reset {
                seed,
                control,
                tier,
                max_ticks,
                scenario,
                factions,
                cadence,
            }) => match Episode::new(
                seed,
                &control,
                tier,
                max_ticks,
                scenario.as_deref(),
                factions.as_deref(),
                cadence,
            ) {
                Ok(mut e) => {
                    let reply = e.reply();
                    episode = Some(e);
                    reply
                }
                Err(err) => serde_json::json!({ "error": format!("{err:#}") }),
            },
            Ok(Request::Step { actions }) => match episode.as_mut() {
                Some(e) if e.live() => match e.step(&actions) {
                    Ok(()) => e.reply(),
                    Err(err) => serde_json::json!({ "error": format!("{err:#}") }),
                },
                Some(_) => serde_json::json!({ "error": "episode is over; reset first" }),
                None => serde_json::json!({ "error": "no episode; reset first" }),
            },
            Ok(Request::Quit) => break,
            Err(err) => serde_json::json!({ "error": format!("bad request: {err}") }),
        };
        writeln!(out, "{reply}")?;
        out.flush()?;
    }
    Ok(())
}

/// Runs the promotion tournament for a quantized artifact: every
/// scripted tier plus the rush canary, `seeds` seeds x both seats,
/// printed as JSON lines. This measures the shipped integer bot — the
/// float checkpoint it came from is a different (unshippable) player.
pub fn neural_cup(
    weights: &std::path::Path,
    seeds: u64,
    scenario: &str,
    profile: NeuralCupProfile,
) -> Result<()> {
    use oxide_sim::bot::{Level, NeuralBot, QuantNet};
    let NeuralCupProfile {
        cadence,
        max_ticks,
        blunder,
        skill,
        aggression,
        factions,
    } = profile;
    let json = std::fs::read_to_string(weights)
        .with_context(|| format!("reading {}", weights.display()))?;
    let net = QuantNet::from_json(&json).map_err(|e| anyhow::anyhow!(e))?;
    let raw_profile = skill.is_some() || blunder.is_some();
    let profile_name = if raw_profile { "raw" } else { "ladder" };
    let effective_cadence = cadence.unwrap_or_else(|| Level::Expert.cadence());
    // Every result line carries the artifact's digest: a cup table
    // pasted into an experiments note answers "which weights" on its
    // own, long after the checkpoint path stops meaning anything.
    let digest = format!("{:016x}", net.digest());
    let (_, actual_factions) = prepare_cup_scenario(scenario, 3000, factions)?;
    let faction_source = if factions.is_some() {
        "override"
    } else {
        "authored"
    };
    eprintln!(
        "artifact: {} · digest {digest} · profile {profile_name} · factions {} ({faction_source})",
        weights.display(),
        actual_factions.code()
    );
    for opponent_kind in [
        CupOpponent::Tier(Difficulty::Scrapheap),
        CupOpponent::Tier(Difficulty::Standard),
        CupOpponent::Tier(Difficulty::Veteran),
        CupOpponent::Tier(Difficulty::Prime),
        CupOpponent::Rusher,
    ] {
        // Every (seed, seat) game is an independent deterministic sim, so
        // they run across threads; aggregation folds a pre-ordered result
        // vector, keeping the printed numbers identical to the serial
        // loop whatever the scheduling.
        let pairs: Vec<(u64, u8)> = (3000..3000 + seeds)
            .flat_map(|seed| [(seed, 0u8), (seed, 1u8)])
            .collect();
        let play = |&(seed, seat): &(u64, u8)| -> Result<CupOutcome> {
            let (sc, game_factions) = prepare_cup_scenario(scenario, seed, factions)?;
            debug_assert_eq!(game_factions, actual_factions);
            let faction = game_factions.faction(seat);
            let mut neural = if raw_profile {
                NeuralBot::with_profile_hesitation(
                    PlayerId(seat),
                    effective_cadence,
                    net.clone(),
                    skill.unwrap_or_else(|| Level::Expert.skill()),
                    aggression,
                    faction,
                    blunder,
                    seed,
                )
            } else if let Some(cadence) = cadence {
                NeuralBot::ladder_with_net_at_cadence(
                    PlayerId(seat),
                    seed,
                    Level::Expert,
                    Some(aggression),
                    faction,
                    net.clone(),
                    cadence,
                )
            } else {
                NeuralBot::ladder_with_net(
                    PlayerId(seat),
                    seed,
                    Level::Expert,
                    Some(aggression),
                    faction,
                    net.clone(),
                )
            };
            let mut opponent = match opponent_kind {
                CupOpponent::Tier(tier) => Some(Brain::for_tier(PlayerId(1 - seat), seed, tier)),
                CupOpponent::Rusher => None,
            };
            let mut rusher = match opponent_kind {
                CupOpponent::Tier(_) => None,
                CupOpponent::Rusher => {
                    Some(GymBot::with_cadence(PlayerId(1 - seat), effective_cadence))
                }
            };
            let sampled = oxide_kit::composition::sample_driven(&sc, max_ticks, 20, |state| {
                let mut commands = neural.act(state);
                if let Some(opponent) = opponent.as_mut() {
                    commands.extend(opponent.act(state));
                }
                if let Some(rusher) = rusher.as_mut()
                    && state.current_tick().is_multiple_of(rusher.cadence())
                {
                    let decision = rusher.decision(state);
                    let plan = cup_rusher_plan(&decision, state.current_tick());
                    commands.extend(rusher.step_plan(state, plan));
                }
                state.tick(&commands)
            })
            .context("sampling cup match")?;
            // Score by seat membership, not team id — a team number
            // only coincides with the seat index on default-team maps.
            let won = sampled.winners.contains(&seat);
            let draw = sampled.winners.is_empty();
            let recent = |tick| sampled.ticks.saturating_sub(tick) <= 2_000;
            let active_cap = sampled.capped
                && (recent(sampled.activity.last_combat_tick)
                    || recent(sampled.activity.last_economy_tick)
                    || recent(sampled.last_progress_tick));
            Ok((won, draw, sampled.capped, active_cap, sampled.ticks))
        };
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(pairs.len().max(1));
        let chunk = pairs.len().div_ceil(threads);
        let mut outcomes: Vec<Result<CupOutcome>> = Vec::with_capacity(pairs.len());
        std::thread::scope(|scope| {
            let handles: Vec<_> = pairs
                .chunks(chunk)
                .map(|slice| scope.spawn(move || slice.iter().map(play).collect::<Vec<_>>()))
                .collect();
            for handle in handles {
                outcomes.extend(handle.join().expect("cup game thread panicked"));
            }
        });
        let (mut wins, mut draws, mut caps, mut active_caps, mut ticks) =
            (0u64, 0u64, 0u64, 0u64, Vec::new());
        let mut seat_wins = [0u64; 2];
        let mut seat_draws = [0u64; 2];
        let mut seat_caps = [0u64; 2];
        let mut seat_active_caps = [0u64; 2];
        let mut seat_ticks = [Vec::new(), Vec::new()];
        for ((_, seat), outcome) in pairs.iter().copied().zip(outcomes) {
            let (won, draw, capped, active_cap, tick) = outcome?;
            wins += u64::from(won);
            draws += u64::from(draw);
            caps += u64::from(capped);
            active_caps += u64::from(active_cap);
            ticks.push(tick);
            seat_wins[usize::from(seat)] += u64::from(won);
            seat_draws[usize::from(seat)] += u64::from(draw);
            seat_caps[usize::from(seat)] += u64::from(capped);
            seat_active_caps[usize::from(seat)] += u64::from(active_cap);
            seat_ticks[usize::from(seat)].push(tick);
        }
        ticks.sort_unstable();
        for values in &mut seat_ticks {
            values.sort_unstable();
        }
        let games = seeds * 2;
        let by_seat: Vec<_> = (0..2)
            .map(|seat| {
                let seat = seat as u8;
                let values = &seat_ticks[usize::from(seat)];
                serde_json::json!({
                    "seat": seat,
                    "faction": faction_name(actual_factions.faction(seat)),
                    "wins": seat_wins[usize::from(seat)],
                    "draws": seat_draws[usize::from(seat)],
                    "caps": seat_caps[usize::from(seat)],
                    "active_caps": seat_active_caps[usize::from(seat)],
                    "games": seeds,
                    "median_ticks": values[values.len() / 2],
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({
                "opponent": opponent_kind.name(),
                "digest": digest,
                "profile": profile_name,
                "factions": actual_factions.code(),
                "factions_source": faction_source,
                "wins": wins,
                "draws": draws,
                "caps": caps,
                "active_caps": active_caps,
                "games": games,
                "max_ticks": max_ticks,
                "median_ticks": ticks[ticks.len() / 2],
                "by_seat": by_seat,
            })
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_sim::UnitId;
    use oxide_sim::bot::Action;
    use oxide_sim::state::Order;

    fn episode(factions: Option<&[Faction]>) -> Episode {
        Episode::new(17, &[0, 1], Difficulty::Veteran, 100, None, factions, 8)
            .expect("skirmish episode")
    }

    #[test]
    fn native_rush_canary_matches_the_training_teacher() {
        let mut decision = Decision {
            features: [0; FEATURE_COUNT],
            mask: [false; ACTION_COUNT],
        };
        for action in [
            Action::TrainHarvester,
            Action::TrainSentinel,
            Action::FormArmy,
            Action::Push,
            Action::Scout,
        ] {
            decision.mask[action as usize] = true;
        }
        let harvesters = FEATURE_NAMES
            .iter()
            .position(|name| *name == "my_harvesters")
            .unwrap();
        let staging = FEATURE_NAMES
            .iter()
            .position(|name| *name == "staging_army_size")
            .unwrap();

        decision.features[harvesters] = 3;
        assert_eq!(
            cup_rusher_plan(&decision, 0),
            ActionPlan {
                production: Action::TrainHarvester,
                construction: Action::NoConstruction,
                operation: Action::FormArmy,
            }
        );

        decision.features[harvesters] = 4;
        decision.features[staging] = 5;
        assert_eq!(
            cup_rusher_plan(&decision, 0),
            ActionPlan {
                production: Action::TrainSentinel,
                construction: Action::NoConstruction,
                operation: Action::Push,
            }
        );

        decision.mask[Action::Push as usize] = false;
        decision.mask[Action::FormArmy as usize] = false;
        assert_eq!(cup_rusher_plan(&decision, 1024).operation, Action::Scout);
    }

    fn wound_sentinel(episode: &mut Episode, player: PlayerId) -> UnitId {
        let (index, id, max_hp) = episode
            .state
            .units()
            .iter()
            .enumerate()
            .find(|(_, unit)| unit.player == player && unit.kind == oxide_sim::UnitKind::Sentinel)
            .map(|(index, unit)| (index, unit.id, unit.kind.stats().max_hp))
            .expect("seat starts with a sentinel");
        let mut value = serde_json::to_value(&episode.state).expect("serialize state");
        value["units"][index]["hp"] = serde_json::json!((max_hp / 3).max(1));
        episode.state = serde_json::from_value(value).expect("valid wounded state");
        id
    }

    fn repair_bay_episode(
        tick: u64,
        hp_deficit: u32,
        scrap: u32,
        welder_progress: u32,
    ) -> (Episode, UnitId, UnitId) {
        let mut scenario = Scenario::skirmish();
        let patient_spec = scenario
            .units
            .iter_mut()
            .find(|unit| unit.player == 0 && unit.kind == oxide_sim::UnitKind::Sentinel)
            .expect("seat starts with a sentinel");
        (patient_spec.x, patient_spec.y) = (8, 7);
        let welder_spec = scenario
            .units
            .iter_mut()
            .find(|unit| unit.player == 0 && unit.kind == oxide_sim::UnitKind::Harvester)
            .expect("seat starts with a harvester");
        (welder_spec.x, welder_spec.y) = (8, 8);
        scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
            player: 0,
            kind: BuildingKind::RepairBay,
            x: 4,
            y: 7,
        });

        let mut episode = episode(None);
        episode.state = scenario.build().expect("repair bay fixture");
        let patient = episode
            .state
            .units()
            .iter()
            .find(|unit| unit.player == PlayerId(0) && unit.kind == oxide_sim::UnitKind::Sentinel)
            .map(|unit| unit.id)
            .expect("positioned patient");
        let welder = episode
            .state
            .units()
            .iter()
            .find(|unit| {
                unit.player == PlayerId(0)
                    && unit.kind == oxide_sim::UnitKind::Harvester
                    && unit.tile() == chassis::grid::TilePos::new(8, 8)
            })
            .map(|unit| unit.id)
            .expect("positioned welder");
        let patient_index = episode
            .state
            .units()
            .iter()
            .position(|unit| unit.id == patient)
            .expect("patient index");
        let welder_index = episode
            .state
            .units()
            .iter()
            .position(|unit| unit.id == welder)
            .expect("welder index");
        let max_hp = oxide_sim::UnitKind::Sentinel.stats().max_hp;
        let mut value = serde_json::to_value(&episode.state).expect("serialize state");
        value["tick"] = serde_json::json!(tick);
        value["units"][patient_index]["hp"] = serde_json::json!(max_hp - hp_deficit);
        value["units"][welder_index]["order"] =
            serde_json::to_value(Order::RepairUnit { unit: patient })
                .expect("serialize repair order");
        value["units"][welder_index]["progress"] = serde_json::json!(welder_progress);
        value["players"][0]["scrap"] = serde_json::json!(scrap);
        episode.state = serde_json::from_value(value).expect("valid repair fixture");
        (episode, patient, welder)
    }

    fn repair_command(welder: UnitId, patient: UnitId) -> PlayerCommand {
        PlayerCommand {
            player: PlayerId(0),
            command: Command::RepairUnit {
                units: vec![welder],
                target: patient,
                queue: false,
            },
        }
    }

    #[test]
    fn hello_and_reset_reply_advertise_zeroed_effect_telemetry() {
        let hello = hello();
        assert_eq!(hello["effect_telemetry"], true);
        assert_eq!(hello["version"], GYM_VERSION);
        assert_eq!(hello["features"], FEATURE_COUNT);
        assert_eq!(hello["actions"], ACTION_COUNT);
        assert_eq!(
            hello["action_heads"],
            serde_json::json!([
                [0, 1, 2, 3, 4, 5, 6, 7, 8],
                [24, 9, 10, 11, 12, 13, 14, 15, 21, 22, 23],
                [25, 16, 17, 18, 19, 20],
            ])
        );

        let mut episode = episode(None);
        let reply = episode.reply();
        assert_eq!(
            reply["effects"],
            serde_json::json!([
                {
                    "seat": 0,
                    "repair_unit_commands": 0,
                    "unit_hp_restored": 0,
                    "repair_unit_hp_restored": 0,
                    "buildings_salvaged": 0,
                    "buildings_completed": [],
                },
                {
                    "seat": 1,
                    "repair_unit_commands": 0,
                    "unit_hp_restored": 0,
                    "repair_unit_hp_restored": 0,
                    "buildings_salvaged": 0,
                    "buildings_completed": [],
                }
            ])
        );
    }

    #[test]
    fn terminal_reply_distinguishes_tick_cap_from_a_game_result() {
        let mut capped = Episode::new(17, &[0, 1], Difficulty::Veteran, 0, None, None, 8)
            .expect("capped skirmish episode");
        let capped_reply = capped.reply();
        assert_eq!(capped_reply["done"], true);
        assert_eq!(capped_reply["truncated"], true);
        assert!(capped_reply["winner"].is_null());

        let mut decided = episode(None);
        decided.tick_with_effects(&[PlayerCommand {
            player: PlayerId(0),
            command: Command::Surrender,
        }]);
        let decided_reply = decided.reply();
        assert_eq!(decided_reply["done"], true);
        assert_eq!(decided_reply["truncated"], false);
        assert_eq!(decided_reply["winners"], serde_json::json!([1]));
    }

    #[test]
    fn repair_effects_accumulate_across_the_whole_decision_interval() {
        let mut episode = Episode::new(17, &[0], Difficulty::Veteran, 300, None, None, 64)
            .expect("skirmish episode");
        wound_sentinel(&mut episode, PlayerId(0));
        let reset = episode.reply();
        assert_eq!(reset["effects"][0]["unit_hp_restored"], 0);

        episode
            .step(&[[
                Action::Idle as usize,
                Action::RepairUnit as usize,
                Action::NoOperation as usize,
            ]])
            .expect("legal repair action");

        assert_eq!(episode.state.current_tick(), 64);
        assert_eq!(episode.effects[0].repair_unit_commands, 1);
        assert!(episode.effects[0].unit_hp_restored > 0);
        assert_eq!(
            episode.effects[0].repair_unit_hp_restored,
            episode.effects[0].unit_hp_restored
        );
        assert!(episode.effects[0].buildings_completed.is_empty());
        let reply = episode.reply();
        assert_eq!(
            reply["effects"][0]["repair_unit_hp_restored"],
            episode.effects[0].repair_unit_hp_restored
        );
    }

    #[test]
    fn stop_then_repair_attributes_a_same_tick_final_weld() {
        let mut scenario = Scenario::skirmish();
        let patient_spec = scenario
            .units
            .iter_mut()
            .find(|unit| unit.player == 0 && unit.kind == oxide_sim::UnitKind::Sentinel)
            .expect("seat starts with a sentinel");
        (patient_spec.x, patient_spec.y) = (8, 7);
        let welder_spec = scenario
            .units
            .iter_mut()
            .find(|unit| unit.player == 0 && unit.kind == oxide_sim::UnitKind::Harvester)
            .expect("seat starts with a harvester");
        (welder_spec.x, welder_spec.y) = (8, 8);

        let mut episode = episode(None);
        episode.state = scenario.build().expect("moving-patient fixture");
        let patient = episode
            .state
            .units()
            .iter()
            .find(|unit| unit.player == PlayerId(0) && unit.kind == oxide_sim::UnitKind::Sentinel)
            .map(|unit| unit.id)
            .expect("positioned patient");
        let welder = episode
            .state
            .units()
            .iter()
            .find(|unit| unit.player == PlayerId(0) && unit.kind == oxide_sim::UnitKind::Harvester)
            .map(|unit| unit.id)
            .expect("positioned welder");
        let patient_index = episode
            .state
            .units()
            .iter()
            .position(|unit| unit.id == patient)
            .expect("patient index");
        let welder_index = episode
            .state
            .units()
            .iter()
            .position(|unit| unit.id == welder)
            .expect("welder index");
        let max_hp = oxide_sim::UnitKind::Sentinel.stats().max_hp;
        let mut value = serde_json::to_value(&episode.state).expect("serialize state");
        value["units"][patient_index]["hp"] = serde_json::json!(max_hp - 1);
        value["players"][0]["scrap"] = serde_json::json!(1_000);
        episode.state = serde_json::from_value(value).expect("valid one-step wound");
        episode.state.tick(&[
            PlayerCommand {
                player: PlayerId(0),
                command: Command::Move {
                    units: vec![patient],
                    goal: chassis::grid::TilePos::new(12, 7),
                    queue: false,
                },
            },
            PlayerCommand {
                player: PlayerId(0),
                command: Command::RepairUnit {
                    units: vec![welder],
                    target: patient,
                    queue: false,
                },
            },
        ]);
        let patient_state = episode.state.unit(patient).expect("living patient");
        assert!(
            patient_state.path.is_some()
                || matches!(
                    patient_state.order,
                    Order::Move { .. } | Order::AttackMove { .. } | Order::Found { .. }
                ),
            "the attribution edge requires a patient moving before the command pair"
        );
        assert!(
            matches!(
                episode.state.unit(welder).expect("living welder").order,
                Order::RepairUnit { unit } if unit == patient
            ),
            "the next RepairUnit command must be a progress-preserving reissue"
        );
        let mut value = serde_json::to_value(&episode.state).expect("serialize state");
        value["units"][welder_index]["progress"] = serde_json::json!(2);
        episode.state = serde_json::from_value(value).expect("valid weld progress");
        let patient_pos = episode.state.unit(patient).expect("patient").pos;
        let welder_state = episode.state.unit(welder).expect("welder");
        assert!(
            welder_state.pos.dist_sq(patient_pos)
                <= oxide_sim::stats::REPAIR_REACH * oxide_sim::stats::REPAIR_REACH,
            "fixture welder must begin in repair reach"
        );
        let commands = [
            PlayerCommand {
                player: PlayerId(0),
                command: Command::Stop {
                    units: vec![patient],
                },
            },
            PlayerCommand {
                player: PlayerId(0),
                command: Command::RepairUnit {
                    units: vec![welder],
                    target: patient,
                    queue: false,
                },
            },
        ];
        let mut mechanical = episode.state.clone();
        let report = mechanical.tick(&commands);
        assert_eq!(
            mechanical.unit(patient).expect("healed patient").hp,
            max_hp,
            "the sim must land the final weld in the same tick"
        );
        assert!(report.events.iter().any(|event| {
            matches!(
                event,
                Event::UnitRepaired {
                    unit,
                    source: UnitRepairSource::FieldWelder { unit: source },
                    amount: 1,
                    ..
                } if *unit == patient && *source == welder
            )
        }));
        episode.tick_with_effects(&commands);

        assert_eq!(episode.effects[0].unit_hp_restored, 1);
        assert_eq!(episode.effects[0].repair_unit_hp_restored, 1);
    }

    #[test]
    fn field_weld_inside_a_bay_is_attributed_between_aura_pulses() {
        let (mut episode, patient, welder) = repair_bay_episode(1, 3, 1_000, 2);
        episode.tick_with_effects(&[repair_command(welder, patient)]);

        assert_eq!(episode.effects[0].unit_hp_restored, 1);
        assert_eq!(episode.effects[0].repair_unit_hp_restored, 1);
        assert_eq!(episode.effects[0].repair_unit_commands, 1);
    }

    #[test]
    fn repair_bay_only_healing_is_not_field_weld_telemetry() {
        let (mut episode, _, _) = repair_bay_episode(0, 3, 1_000, 0);
        episode.tick_with_effects(&[]);

        assert_eq!(episode.effects[0].unit_hp_restored, 1);
        assert_eq!(episode.effects[0].repair_unit_hp_restored, 0);
        assert_eq!(episode.effects[0].repair_unit_commands, 0);
    }

    #[test]
    fn overlapping_field_and_bay_repairs_report_each_accepted_source() {
        let (mut episode, patient, welder) = repair_bay_episode(0, 3, 1_000, 2);
        episode.tick_with_effects(&[repair_command(welder, patient)]);

        assert_eq!(episode.effects[0].unit_hp_restored, 2);
        assert_eq!(episode.effects[0].repair_unit_hp_restored, 1);
        assert_eq!(episode.effects[0].repair_unit_commands, 1);
    }

    #[test]
    fn unaffordable_bay_pulse_does_not_hide_an_accepted_field_weld() {
        let (mut episode, patient, welder) = repair_bay_episode(0, 3, 2, 2);
        episode.tick_with_effects(&[repair_command(welder, patient)]);

        assert_eq!(episode.effects[0].unit_hp_restored, 1);
        assert_eq!(episode.effects[0].repair_unit_hp_restored, 1);
        assert_eq!(episode.state.player(PlayerId(0)).scrap, 0);
    }

    #[test]
    fn insufficient_scrap_reports_neither_unperformed_repair_source() {
        let (mut bay, _, _) = repair_bay_episode(0, 1, 0, 0);
        bay.tick_with_effects(&[]);
        assert_eq!(bay.effects[0].unit_hp_restored, 0);
        assert_eq!(bay.effects[0].repair_unit_hp_restored, 0);

        let (mut field, patient, welder) = repair_bay_episode(1, 1, 0, 2);
        field.tick_with_effects(&[repair_command(welder, patient)]);
        assert_eq!(field.effects[0].unit_hp_restored, 0);
        assert_eq!(field.effects[0].repair_unit_hp_restored, 0);
        assert_eq!(field.effects[0].repair_unit_commands, 1);
    }

    #[test]
    fn completed_and_salvaged_building_effects_are_exact_and_seat_scoped() {
        let mut episode = episode(None);
        episode.note_events(&[
            Event::BuildingCompleted {
                building: oxide_sim::BuildingId(90),
                player: PlayerId(1),
                kind: BuildingKind::RepairBay,
            },
            Event::BuildingCompleted {
                building: oxide_sim::BuildingId(91),
                player: PlayerId(0),
                kind: BuildingKind::Array,
            },
            Event::BuildingCompleted {
                building: oxide_sim::BuildingId(92),
                player: PlayerId(0),
                kind: BuildingKind::Turret,
            },
            Event::BuildingSalvaged {
                building: oxide_sim::BuildingId(93),
                player: PlayerId(0),
                pos: chassis::fx::Vec2Fx::ZERO,
                refund: 80,
            },
            Event::BuildingSalvaged {
                building: oxide_sim::BuildingId(94),
                player: PlayerId(0),
                pos: chassis::fx::Vec2Fx::ZERO,
                refund: 40,
            },
            Event::BuildingSalvaged {
                building: oxide_sim::BuildingId(95),
                player: PlayerId(1),
                pos: chassis::fx::Vec2Fx::ZERO,
                refund: 80,
            },
        ]);
        let reply = episode.reply();
        assert_eq!(
            reply["effects"][0]["buildings_completed"],
            serde_json::json!(["array", "turret"])
        );
        assert_eq!(
            reply["effects"][1]["buildings_completed"],
            serde_json::json!(["repair_bay"])
        );
        assert_eq!(reply["effects"][0]["buildings_salvaged"], 2);
        assert_eq!(reply["effects"][1]["buildings_salvaged"], 1);
    }

    #[test]
    fn every_duel_faction_pair_parses_canonically() {
        for (text, expected) in [
            ("ff", [Faction::Ferrous, Faction::Ferrous]),
            ("FC", [Faction::Ferrous, Faction::Cupric]),
            ("cf", [Faction::Cupric, Faction::Ferrous]),
            ("cc", [Faction::Cupric, Faction::Cupric]),
        ] {
            let pair = text.parse::<DuelFactions>().expect("valid pair");
            assert_eq!(pair.0, expected);
            assert_eq!(pair.code(), text.to_ascii_lowercase());
        }
    }

    #[test]
    fn invalid_duel_faction_pairs_are_rejected() {
        for text in ["", "f", "fcc", "fx", "ferrous-cupric"] {
            let err = text
                .parse::<DuelFactions>()
                .expect_err("invalid faction pair");
            assert!(
                err.contains("expected ff, fc, cf, or cc"),
                "unexpected error for {text:?}: {err}"
            );
        }
    }

    #[test]
    fn cup_overrides_cover_both_factions_from_both_physical_seats() {
        let mut seen = [[false; 2]; 2];
        for text in ["ff", "fc", "cf", "cc"] {
            let requested = text.parse::<DuelFactions>().expect("valid pair");
            let (scenario, actual) = prepare_cup_scenario("skirmish", 41, Some(requested))
                .expect("retinted cup scenario");
            assert_eq!(actual, requested);
            for (seat, seen_factions) in seen.iter_mut().enumerate() {
                let faction = scenario.players[seat].faction;
                assert_eq!(faction, requested.0[seat]);
                seen_factions[usize::from(faction == Faction::Cupric)] = true;
            }
        }
        assert_eq!(seen, [[true, true], [true, true]]);
    }

    #[test]
    fn omitted_cup_factions_preserve_the_scenario_exactly() {
        let mut authored = Scenario::skirmish();
        authored.seed = 41;
        let (prepared, actual) =
            prepare_cup_scenario("skirmish", 41, None).expect("authored cup scenario");
        assert_eq!(prepared, authored);
        assert_eq!(actual.code(), "fc");
    }

    #[test]
    fn cup_rejects_non_duel_scenarios_before_building() {
        let mut scenario = Scenario::skirmish();
        scenario.players.push(scenario.players[0].clone());
        let err = DuelFactions::from_scenario(&scenario).expect_err("three-seat cup");
        assert!(
            err.to_string()
                .contains("neural-cup requires a 2-seat duel scenario, got 3 seats"),
            "{err:#}"
        );
    }

    #[test]
    fn reset_factions_are_optional_in_the_wire_contract() {
        let old = serde_json::from_str::<Request>(
            r#"{"cmd":"reset","seed":1,"control":[0,1],"max_ticks":100}"#,
        )
        .expect("old reset request");
        match old {
            Request::Reset { factions, .. } => assert!(factions.is_none()),
            _ => panic!("parsed the wrong request"),
        }

        let new = serde_json::from_str::<Request>(
            r#"{"cmd":"reset","seed":1,"control":[0,1],"factions":["cupric","ferrous"]}"#,
        )
        .expect("retinted reset request");
        match new {
            Request::Reset { factions, .. } => {
                assert_eq!(factions, Some(vec![Faction::Cupric, Faction::Ferrous]))
            }
            _ => panic!("parsed the wrong request"),
        }
    }

    #[test]
    fn step_wire_requires_one_three_head_plan_per_seat() {
        let request =
            serde_json::from_str::<Request>(r#"{"cmd":"step","actions":[[2,13,18],[0,24,25]]}"#)
                .expect("factorized step request");
        match request {
            Request::Step { actions } => assert_eq!(actions, vec![[2, 13, 18], [0, 24, 25]]),
            _ => panic!("parsed the wrong request"),
        }
        assert!(
            serde_json::from_str::<Request>(r#"{"cmd":"step","actions":[2,13]}"#).is_err(),
            "the flat v6 wire shape must fail at the contract boundary"
        );
    }

    #[test]
    fn every_two_seat_faction_pair_reaches_state_and_observation() {
        let faction_feature = oxide_sim::bot::FEATURE_NAMES
            .iter()
            .position(|name| *name == "faction")
            .expect("gym contract carries faction");
        for factions in [
            [Faction::Ferrous, Faction::Ferrous],
            [Faction::Ferrous, Faction::Cupric],
            [Faction::Cupric, Faction::Ferrous],
            [Faction::Cupric, Faction::Cupric],
        ] {
            let mut episode = episode(Some(&factions));
            assert_eq!(episode.state.player(PlayerId(0)).faction, factions[0]);
            assert_eq!(episode.state.player(PlayerId(1)).faction, factions[1]);

            let reply = episode.reply();
            assert_eq!(
                reply["factions"],
                serde_json::json!([factions[0], factions[1]])
            );
            let seats = reply["seats"].as_array().expect("seat observations");
            for (seat, faction) in factions.into_iter().enumerate() {
                let view = seats
                    .iter()
                    .find(|view| view["seat"] == seat as u8)
                    .expect("controlled seat");
                assert_eq!(
                    view["features"][faction_feature],
                    i64::from(faction == Faction::Cupric),
                    "seat {seat} observation must report its retinted faction"
                );
            }
        }
    }

    #[test]
    fn omitted_factions_preserve_the_authored_rosters() {
        let episode = episode(None);
        assert_eq!(episode.state.player(PlayerId(0)).faction, Faction::Ferrous);
        assert_eq!(episode.state.player(PlayerId(1)).faction, Faction::Cupric);
    }

    #[test]
    fn a_partial_faction_list_is_rejected_instead_of_misapplied() {
        let err = Episode::new(
            17,
            &[0],
            Difficulty::Veteran,
            100,
            None,
            Some(&[Faction::Cupric]),
            8,
        )
        .err()
        .expect("one faction cannot describe a two-seat scenario");
        assert!(
            err.to_string()
                .contains("factions must name exactly 2 seats, got 1"),
            "{err:#}"
        );
    }
}
