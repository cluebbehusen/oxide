//! The training loop's other half: episodes over stdio.
//!
//! `oxide-driver gym` speaks newline-delimited JSON on stdin/stdout so
//! a trainer (see `tools/train/`) can drive [`GymBot`] episodes without
//! linking Rust: reset with a seed, seat, and opponent tier; receive
//! integer features and an action mask at every decision tick; send an
//! action index back. One process, many sequential episodes — the
//! trainer runs several processes for parallelism. Determinism holds
//! the whole way down: same seed and same actions replay the same
//! match, which is what makes training rollouts auditable.

use anyhow::{Context, Result, bail};
use oxide_sim::bot::{ACTION_COUNT, Action, Brain, Difficulty, FEATURE_COUNT, GYM_VERSION, GymBot};
use oxide_sim::state::GameResult;
use oxide_sim::{PlayerId, State};
use serde::Deserialize;
use std::io::{BufRead, Write};

#[derive(Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
enum Request {
    Reset {
        seed: u64,
        #[serde(default)]
        seat: u8,
        #[serde(default = "default_tier")]
        tier: Difficulty,
        #[serde(default = "default_max_ticks")]
        max_ticks: u64,
        #[serde(default)]
        scenario: Option<String>,
    },
    Step {
        action: usize,
    },
    Quit,
}

fn default_tier() -> Difficulty {
    Difficulty::Veteran
}

fn default_max_ticks() -> u64 {
    40_000
}

struct Episode {
    state: State,
    gym: GymBot,
    opponent: Brain,
    seat: PlayerId,
    max_ticks: u64,
}

impl Episode {
    fn new(
        seed: u64,
        seat: u8,
        tier: Difficulty,
        max_ticks: u64,
        scenario: Option<&str>,
    ) -> Result<Self> {
        if seat > 1 {
            bail!("seat must be 0 or 1");
        }
        let mut scenario = crate::runner::load_scenario(scenario.unwrap_or("skirmish"))?;
        scenario.seed = seed;
        let state = scenario.build().context("scenario build")?;
        Ok(Self {
            state,
            gym: GymBot::new(PlayerId(seat)),
            opponent: Brain::for_tier(PlayerId(1 - seat), seed, tier),
            seat: PlayerId(seat),
            max_ticks,
        })
    }

    /// True while the match is live and under the tick cap.
    fn live(&self) -> bool {
        self.state.result().is_none() && self.state.current_tick() < self.max_ticks
    }

    /// Applies the trainer's action at the current decision tick, then
    /// advances to the next decision tick (or the end).
    fn step(&mut self, action: usize) {
        let mut commands = self.gym.step(&self.state, Action::from_index(action));
        commands.extend(self.opponent.act(&self.state));
        self.state.tick(&commands);
        while self.live() && !self.state.current_tick().is_multiple_of(self.gym.cadence()) {
            let commands = self.opponent.act(&self.state);
            self.state.tick(&commands);
        }
    }

    fn reply(&self) -> serde_json::Value {
        if self.live() {
            let d = self.gym.decision(&self.state);
            serde_json::json!({
                "done": false,
                "tick": self.state.current_tick(),
                "features": d.features.to_vec(),
                "mask": d.mask.to_vec(),
            })
        } else {
            let win = match self.state.result() {
                Some(GameResult::Victory { winner }) => Some(winner == self.seat),
                _ => None,
            };
            serde_json::json!({
                "done": true,
                "tick": self.state.current_tick(),
                "win": win,
            })
        }
    }
}

/// Runs the stdio loop until EOF or a quit command.
pub fn serve() -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(
        out,
        "{}",
        serde_json::json!({
            "ready": true,
            "version": GYM_VERSION,
            "features": FEATURE_COUNT,
            "actions": ACTION_COUNT,
        })
    )?;
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
                seat,
                tier,
                max_ticks,
                scenario,
            }) => match Episode::new(seed, seat, tier, max_ticks, scenario.as_deref()) {
                Ok(e) => {
                    let reply = e.reply();
                    episode = Some(e);
                    reply
                }
                Err(err) => serde_json::json!({ "error": format!("{err:#}") }),
            },
            Ok(Request::Step { action }) => match episode.as_mut() {
                Some(e) if e.live() => {
                    e.step(action);
                    e.reply()
                }
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
