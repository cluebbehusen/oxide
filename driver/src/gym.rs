//! The training loop's other half: episodes over stdio.
//!
//! `oxide-driver gym` speaks newline-delimited JSON on stdin/stdout so
//! a trainer (see `tools/train/`) can drive [`GymBot`] episodes without
//! linking Rust. `control` names the externally-driven seats: one seat
//! against a scripted tier for curriculum and evaluation, or both
//! seats for self-play and league play — every decision tick then
//! carries features and masks for each controlled seat, and `step`
//! takes one action per controlled seat, in the same order.
//! Determinism holds the whole way down: same seed and same actions
//! replay the same match, which is what makes rollouts auditable.

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
    },
    Step {
        /// One action per controlled seat, in `control` order.
        actions: Vec<usize>,
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

struct Episode {
    state: State,
    gyms: Vec<GymBot>,
    opponent: Option<Brain>,
    max_ticks: u64,
}

impl Episode {
    fn new(
        seed: u64,
        control: &[u8],
        tier: Difficulty,
        max_ticks: u64,
        scenario: Option<&str>,
    ) -> Result<Self> {
        if control.is_empty() || control.len() > 2 {
            bail!("control must name one or two seats");
        }
        if control.iter().any(|s| *s > 1) || (control.len() == 2 && control[0] == control[1]) {
            bail!("controlled seats must be distinct 0/1");
        }
        let mut scenario = crate::runner::load_scenario(scenario.unwrap_or("skirmish"))?;
        scenario.seed = seed;
        let state = scenario.build().context("scenario build")?;
        let gyms: Vec<GymBot> = control.iter().map(|s| GymBot::new(PlayerId(*s))).collect();
        let opponent =
            (control.len() == 1).then(|| Brain::for_tier(PlayerId(1 - control[0]), seed, tier));
        Ok(Self {
            state,
            gyms,
            opponent,
            max_ticks,
        })
    }

    /// True while the match is live and under the tick cap.
    fn live(&self) -> bool {
        self.state.result().is_none() && self.state.current_tick() < self.max_ticks
    }

    fn cadence(&self) -> u64 {
        self.gyms[0].cadence()
    }

    /// Applies the trainer's actions at the current decision tick, then
    /// advances to the next decision tick (or the end).
    fn step(&mut self, actions: &[usize]) -> Result<()> {
        if actions.len() != self.gyms.len() {
            bail!(
                "expected {} actions (one per controlled seat), got {}",
                self.gyms.len(),
                actions.len()
            );
        }
        let mut commands = Vec::new();
        for (gym, action) in self.gyms.iter_mut().zip(actions) {
            commands.extend(gym.step(&self.state, Action::from_index(*action)));
        }
        if let Some(op) = self.opponent.as_mut() {
            commands.extend(op.act(&self.state));
        }
        self.state.tick(&commands);
        while self.live() && !self.state.current_tick().is_multiple_of(self.cadence()) {
            let commands = self
                .opponent
                .as_mut()
                .map(|op| op.act(&self.state))
                .unwrap_or_default();
            self.state.tick(&commands);
        }
        Ok(())
    }

    fn reply(&mut self) -> serde_json::Value {
        if self.live() {
            let state = &self.state;
            let seats: Vec<_> = self
                .gyms
                .iter_mut()
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
            })
        } else {
            let winner = match self.state.result() {
                Some(GameResult::Victory { winner }) => Some(winner.0),
                _ => None,
            };
            serde_json::json!({
                "done": true,
                "tick": self.state.current_tick(),
                "winner": winner,
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
                control,
                tier,
                max_ticks,
                scenario,
            }) => match Episode::new(seed, &control, tier, max_ticks, scenario.as_deref()) {
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
