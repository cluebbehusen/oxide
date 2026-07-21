//! Deterministic integer inference for a trained policy.
//!
//! The sim may not touch floats, so a shipped network is a fixed-point
//! artifact produced by `tools/train/export.py`: Q12 weights, a Q12
//! tanh lookup table, per-feature reciprocal scales. Everything here is
//! `i64` adds, multiplies, and shifts — bit-identical on every platform,
//! which is what lets a neural tier live inside replays and hash
//! fixtures like any other command source. The quantized network is a
//! slightly different player than the float one that trained (12
//! fractional bits), so promotion tournaments run against *this* bot,
//! not the torch checkpoint.

use super::gym::{ACTION_COUNT, Action, Decision, FEATURE_COUNT, GYM_VERSION, GymBot};
use crate::command::PlayerCommand;
use crate::ids::PlayerId;
use crate::state::State;
use chassis::rng::Pcg32;
use serde::Deserialize;

/// Fractional bits of the fixed-point format.
const Q: u32 = 12;
/// The tanh table spans [-8, 8] in Q12: ±(8 << 12).
const TANH_SPAN: i64 = 8 << Q;

#[derive(Deserialize)]
struct LayerDto {
    w: Vec<Vec<i32>>,
    b: Vec<i32>,
}

#[derive(Deserialize)]
struct ArtifactDto {
    gym_version: u32,
    q_bits: u32,
    features: usize,
    actions: usize,
    recips: Vec<i64>,
    tanh_lut: Vec<i32>,
    layers: Vec<LayerDto>,
    head: LayerDto,
}

/// A quantized policy network: trunk of tanh layers plus a logit head.
#[derive(Debug, Clone)]
pub struct QuantNet {
    recips: Vec<i64>,
    tanh_lut: Vec<i32>,
    layers: Vec<(Vec<Vec<i32>>, Vec<i32>)>,
    head: (Vec<Vec<i32>>, Vec<i32>),
}

impl QuantNet {
    /// Parses an exported artifact, refusing shape or version drift.
    pub fn from_json(json: &str) -> Result<Self, String> {
        let dto: ArtifactDto = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if dto.gym_version != GYM_VERSION {
            return Err(format!(
                "weights speak gym v{}, sim speaks v{GYM_VERSION}",
                dto.gym_version
            ));
        }
        if dto.q_bits != Q {
            return Err(format!("weights use Q{}, sim uses Q{Q}", dto.q_bits));
        }
        if dto.features != FEATURE_COUNT || dto.actions != ACTION_COUNT {
            return Err("feature/action shape mismatch".into());
        }
        if dto.recips.len() != FEATURE_COUNT || dto.tanh_lut.len() != 513 {
            return Err("scale or lut shape mismatch".into());
        }
        let mut width = FEATURE_COUNT;
        for (i, l) in dto
            .layers
            .iter()
            .chain(std::iter::once(&dto.head))
            .enumerate()
        {
            if l.w.is_empty() || l.w.iter().any(|row| row.len() != width) || l.b.len() != l.w.len()
            {
                return Err(format!("layer {i} shape mismatch"));
            }
            width = l.w.len();
        }
        if width != ACTION_COUNT {
            return Err("head does not produce one logit per action".into());
        }
        Ok(Self {
            recips: dto.recips,
            tanh_lut: dto.tanh_lut,
            layers: dto.layers.into_iter().map(|l| (l.w, l.b)).collect(),
            head: (dto.head.w, dto.head.b),
        })
    }

    /// Q12 tanh via table lookup with linear interpolation.
    fn tanh(&self, x: i64) -> i64 {
        let x = x.clamp(-TANH_SPAN, TANH_SPAN);
        let pos = x + TANH_SPAN; // 0..=2*TANH_SPAN (65536)
        let idx = (pos >> 7) as usize; // 512 buckets of 128
        let frac = pos & 127;
        let a = i64::from(self.tanh_lut[idx.min(512)]);
        let b = i64::from(self.tanh_lut[(idx + 1).min(512)]);
        a + (((b - a) * frac) >> 7)
    }

    fn affine(w: &[Vec<i32>], b: &[i32], input: &[i64]) -> Vec<i64> {
        w.iter()
            .zip(b)
            .map(|(row, bias)| {
                let acc: i64 = row
                    .iter()
                    .zip(input)
                    .map(|(wi, xi)| i64::from(*wi) * xi)
                    .sum();
                (acc >> Q) + i64::from(*bias)
            })
            .collect()
    }

    /// Q12 logits for a raw integer feature vector.
    pub fn logits(&self, features: &[i64; FEATURE_COUNT]) -> Vec<i64> {
        let mut act: Vec<i64> = features
            .iter()
            .zip(&self.recips)
            .map(|(f, r)| (f * r) >> Q)
            .collect();
        for (w, b) in &self.layers {
            act = Self::affine(w, b, &act)
                .into_iter()
                .map(|x| self.tanh(x))
                .collect();
        }
        Self::affine(&self.head.0, &self.head.1, &act)
    }

    /// Greedy masked action: the highest-logit legal action, ties to
    /// the lowest index; Idle if somehow nothing is legal.
    pub fn act(&self, decision: &Decision) -> Action {
        let logits = self.logits(&decision.features);
        let mut best: Option<(i64, usize)> = None;
        for (i, legal) in decision.mask.iter().enumerate() {
            if !legal {
                continue;
            }
            if best.is_none_or(|(v, _)| logits[i] > v) {
                best = Some((logits[i], i));
            }
        }
        Action::from_index(best.map_or(0, |(_, i)| i))
    }
}

/// A trained policy as a command source: [`GymBot`] chores and
/// executive, network decisions. Deterministic end to end.
///
/// Difficulty is a play-time dial, not a different network: with
/// probability `blunder` (per mille) a decision is a uniformly random
/// *legal* action instead of the network's best. The mistakes stay
/// human-shaped — the bot still builds, defends, and attacks, it just
/// misjudges — and the dial is continuous, which is what a dynamic
/// difficulty needs. Fair by construction: it changes thinking, never
/// income or vision.
#[derive(Debug, Clone)]
pub struct NeuralBot {
    gym: GymBot,
    net: QuantNet,
    blunder_permille: u32,
    rng: Pcg32,
}

impl NeuralBot {
    /// A full-strength neural bot for `player` deciding every `cadence`
    /// ticks (use the cadence the network trained at).
    pub fn new(player: PlayerId, cadence: u64, net: QuantNet) -> Self {
        Self::with_blunder(player, cadence, net, 0, 0)
    }

    /// A dialed-down bot: `blunder_permille` in 0..=1000. The rng is
    /// seeded from the scenario like every other bot — replays hold.
    pub fn with_blunder(
        player: PlayerId,
        cadence: u64,
        net: QuantNet,
        blunder_permille: u32,
        scenario_seed: u64,
    ) -> Self {
        Self {
            gym: GymBot::with_cadence(player, cadence),
            net,
            blunder_permille: blunder_permille.min(1000),
            rng: Pcg32::new(scenario_seed, 3000 + u64::from(player.0)),
        }
    }

    /// The player this bot drives.
    pub fn player(&self) -> PlayerId {
        self.gym.player()
    }

    /// Commands for this tick (empty off the think cadence).
    pub fn act(&mut self, state: &State) -> Vec<PlayerCommand> {
        if state.result().is_some() || !state.current_tick().is_multiple_of(self.gym.cadence()) {
            return Vec::new();
        }
        let decision = self.gym.decision(state);
        let mut action = self.net.act(&decision);
        if self.blunder_permille > 0 && self.rng.next_below(1000) < self.blunder_permille {
            // A blunder is a *plausible* mistake — the second- or
            // third-best idea, not a uniformly random legal action. In a
            // macro action space one mad decision loses a game outright,
            // and uniform blunders turn the dial into a cliff; near-best
            // blunders make it a slope.
            let logits = self.net.logits(&decision.features);
            let mut ranked: Vec<(i64, usize)> = decision
                .mask
                .iter()
                .enumerate()
                .filter(|(_, ok)| **ok)
                .map(|(i, _)| (logits[i], i))
                .collect();
            ranked.sort_unstable_by_key(|(v, i)| (-*v, *i));
            if ranked.len() > 1 {
                let alternates = (ranked.len() - 1).min(2) as u32;
                let pick = ranked[1 + self.rng.next_below(alternates) as usize].1;
                action = Action::from_index(pick);
            }
        }
        self.gym.step(state, action)
    }
}
