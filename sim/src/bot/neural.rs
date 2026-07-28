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
use crate::state::Faction;
use crate::state::State;
use chassis::rng::Pcg32;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// The decision cadence the shipped ladder network trained at.
pub const LADDER_CADENCE: u64 = 16;

/// Floor of the seed-dealt personality range. An explicit `aggression`
/// pick may use the full 0..=1000 conditioning the network trained
/// under; the deal narrows it. Below this floor the trained style is a
/// deep turtle — paid during training for its army NOT fighting — and
/// a dealt one reads as a bot that never attacks (the 0.12 playtest
/// complaint; the open deal measured 15/48 undecided on the skirmish
/// sweep). The extremes stay reachable through explicit picks, on
/// purpose.
pub const DEALT_AGGRESSION_MIN: u32 = 250;
/// Ceiling of the seed-dealt personality range — trims the mirror
/// extreme of [`DEALT_AGGRESSION_MIN`]'s turtle.
pub const DEALT_AGGRESSION_MAX: u32 = 900;

/// Deals the personality a seat plays when its scenario config leaves
/// `aggression` unset: uniform in the dealt range, deterministic from
/// the scenario seed. The one definition — driver probes call this
/// instead of replicating the stream.
pub fn deal_aggression(scenario_seed: u64, player: PlayerId) -> u32 {
    DEALT_AGGRESSION_MIN
        + Pcg32::new(scenario_seed, 4000 + u64::from(player.0))
            .next_below(DEALT_AGGRESSION_MAX - DEALT_AGGRESSION_MIN + 1)
}

/// The shipped difficulty ladder: one trained network, four skill-knob
/// settings calibrated against the scripted tiers (Easy loses to even
/// the gentlest scripted bot; Expert sweeps them all). Difficulty is a
/// dial into one mind, not four different minds — mistakes stay
/// human-shaped at every rung.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Level {
    /// Beatable while learning the controls.
    Easy,
    /// A fair first fight.
    Medium,
    /// Wins against casual play.
    Hard,
    /// The full network, no handicap.
    Expert,
}

impl Level {
    /// All levels, gentlest first.
    pub const LADDER: [Level; 4] = [Level::Easy, Level::Medium, Level::Hard, Level::Expert];

    /// The skill-knob setting this level plays at. Recalibrated for the
    /// 0.10 artifact under hesitation blunders (Easy dithers away 35%
    /// of its decision windows, Medium 19%, Hard 7.5%). Every rung
    /// keeps the varied full-tech game — that invariance is the point
    /// of the hesitation model; the old wrong-action blunders spent
    /// the Fabricator fund and collapsed every degraded level into
    /// sentinel spam.
    pub fn skill(self) -> u32 {
        match self {
            Level::Easy => 300,
            Level::Medium => 620,
            Level::Hard => 850,
            Level::Expert => 1000,
        }
    }

    /// How often this level thinks, in ticks — the second difficulty
    /// dial: lower minds think slower. Calibrated against the scripted
    /// yardsticks, because head-to-head mirrors stopped ordering under
    /// the 0.10 balance: patience wins there, so a slower thinker
    /// turtles into a tech advantage and "handicaps" cancel out.
    /// Against aggression — scripted tiers, human rushes — reaction
    /// lag costs what it should. The in-tree ladder test is the
    /// 80-match pace-ordering tripwire; `driver yardstick` is the
    /// wide instrument (0.12 read: 34 < 42 < 46 < 48 of 48, and every
    /// probed "stronger Medium" dial measured weaker — the skill knob
    /// is trained conditioning, not a pure handicap, so these dials
    /// sit at a measured local optimum; see the 0.12 experiments
    /// note before moving them).
    pub fn cadence(self) -> u64 {
        match self {
            Level::Easy => 56,
            // 36 until 0.12: at 36, symmetric Medium mirrors stalled
            // 14/48 on skirmish; at 26 that drops to 2/48 at par
            // yardstick strength (81 vs 82 of 96) and decisions land
            // ~15% sooner. The response is non-monotonic — 30
            // measured WORSE on both instruments (32/48, and it
            // resurrected the Standard-stall blemish) — so don't
            // interpolate; re-measure.
            Level::Medium => 26,
            Level::Hard => 24,
            Level::Expert => 16,
        }
    }
}

/// Fractional bits of the fixed-point format.
const Q: u32 = 12;
/// The tanh table spans [-8, 8] in Q12: ±(8 << 12).
const TANH_SPAN: i64 = 8 << Q;

// The artifact's numeric contract. `from_json` is a trust boundary,
// not just a shape check — `--weights` loads files the sim did not
// write — and the kernel is `i64` throughout, so every tensor carries
// a magnitude ceiling and the accumulators are bounded by
// construction:
//
//   scaling: |feature| <= MAX_FEATURE (2^36) times |recip| <=
//     MAX_RECIP (2^26) is 2^62, and the shifted result enters the
//     trunk clamped to MAX_ACT (2^30).
//   affine row: MAX_WIDTH (2^12) terms of |w| <= MAX_COEFF (2^20)
//     times |x| <= MAX_ACT (2^30) is 2^62. Past the first layer |x|
//     is a tanh output, itself capped at MAX_LUT (2^13), so later
//     rows sit near 2^45.
//
// Both peaks leave a bit of headroom under `i64::MAX`, and every
// ceiling sits orders of magnitude above what `tools/train/export.py`
// can structurally emit (the shipped artifact peaks at recip 2^24,
// |lut| 2^12, |w| 2^12, three 384-wide layers), so an honestly
// exported candidate can never be refused and neither clamp can bind
// on a reachable observation. The exporter mirrors these ceilings.

/// Largest accepted per-feature reciprocal scale.
const MAX_RECIP: i64 = 1 << 26;
/// Largest accepted tanh table magnitude.
const MAX_LUT: i32 = 1 << 13;
/// Largest accepted weight or bias magnitude.
const MAX_COEFF: i32 = 1 << 20;
/// Most trunk layers an artifact may declare (the head is extra).
const MAX_LAYERS: usize = 16;
/// Widest layer — and widest input — an artifact may declare.
const MAX_WIDTH: usize = 4096;
/// Raw feature magnitude the scaling step saturates at.
const MAX_FEATURE: i64 = 1 << 36;
/// Scaled activation magnitude the trunk's input saturates at.
const MAX_ACT: i64 = 1 << 30;

#[derive(Deserialize, Serialize)]
struct LayerDto {
    w: Vec<Vec<i32>>,
    b: Vec<i32>,
}

/// What a [`QuantNet::digest`] fingerprints: the parsed tensors and
/// the contract they were accepted under. Reformatting the JSON or
/// editing its `arch`/`update` metadata leaves the digest alone; one
/// changed coefficient moves it.
#[derive(Serialize)]
struct DigestView<'a> {
    gym_version: u32,
    q_bits: u32,
    features: usize,
    conditioning: usize,
    actions: usize,
    recips: &'a [i64],
    tanh_lut: &'a [i32],
    layers: &'a [LayerDto],
    head: &'a LayerDto,
}

#[derive(Deserialize)]
struct ArtifactDto {
    gym_version: u32,
    q_bits: u32,
    features: usize,
    /// Conditioning knobs appended after the gym features (skill,
    /// aggression — 0 for unconditioned artifacts).
    #[serde(default)]
    conditioning: usize,
    actions: usize,
    recips: Vec<i64>,
    tanh_lut: Vec<i32>,
    layers: Vec<LayerDto>,
    head: LayerDto,
}

/// A quantized policy network: trunk of tanh layers plus a logit head.
#[derive(Debug, Clone)]
pub struct QuantNet {
    conditioning: usize,
    recips: Vec<i64>,
    tanh_lut: Vec<i32>,
    layers: Vec<(Vec<Vec<i32>>, Vec<i32>)>,
    head: (Vec<Vec<i32>>, Vec<i32>),
    digest: u64,
}

impl QuantNet {
    /// The embedded ladder network (parsed once, shared).
    ///
    /// # Panics
    /// If the embedded artifact doesn't match this build's gym
    /// contract — a build error surfaced at first use, caught by tests.
    pub fn ladder() -> &'static QuantNet {
        static NET: OnceLock<QuantNet> = OnceLock::new();
        NET.get_or_init(|| {
            QuantNet::from_json(include_str!("ladder_weights.json"))
                .expect("embedded ladder weights match the gym contract")
        })
    }

    /// Parses an exported artifact, refusing version, shape, or
    /// magnitude drift. The magnitude ceilings are what make the
    /// integer kernel total: see the numeric contract above.
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
        let input_width = FEATURE_COUNT + dto.conditioning;
        if dto.recips.len() != input_width || dto.tanh_lut.len() != 513 {
            return Err("scale or lut shape mismatch".into());
        }
        if input_width > MAX_WIDTH {
            return Err(format!(
                "input is {input_width} wide, over the {MAX_WIDTH} ceiling"
            ));
        }
        if dto.layers.len() > MAX_LAYERS {
            return Err(format!(
                "{} trunk layers, over the {MAX_LAYERS} ceiling",
                dto.layers.len()
            ));
        }
        if let Some((i, r)) = dto
            .recips
            .iter()
            .enumerate()
            .find(|(_, r)| **r < 1 || **r > MAX_RECIP)
        {
            return Err(format!("recips[{i}] = {r} outside 1..={MAX_RECIP}"));
        }
        if let Some((i, v)) = dto
            .tanh_lut
            .iter()
            .enumerate()
            .find(|(_, v)| v.unsigned_abs() > MAX_LUT.unsigned_abs())
        {
            return Err(format!("tanh_lut[{i}] = {v} over the +/-{MAX_LUT} ceiling"));
        }
        let mut width = input_width;
        for (i, l) in dto
            .layers
            .iter()
            .chain(std::iter::once(&dto.head))
            .enumerate()
        {
            let name = if i < dto.layers.len() {
                format!("layer {i}")
            } else {
                "head".to_string()
            };
            if l.w.is_empty() || l.w.iter().any(|row| row.len() != width) || l.b.len() != l.w.len()
            {
                return Err(format!("{name} shape mismatch"));
            }
            if l.w.len() > MAX_WIDTH {
                return Err(format!(
                    "{name} is {} wide, over the {MAX_WIDTH} ceiling",
                    l.w.len()
                ));
            }
            if let Some((row, col, v)) = l.w.iter().enumerate().find_map(|(row, r)| {
                r.iter()
                    .enumerate()
                    .find(|(_, v)| v.unsigned_abs() > MAX_COEFF.unsigned_abs())
                    .map(|(col, v)| (row, col, *v))
            }) {
                return Err(format!(
                    "{name} w[{row}][{col}] = {v} over the +/-{MAX_COEFF} ceiling"
                ));
            }
            if let Some((row, v)) =
                l.b.iter()
                    .enumerate()
                    .find(|(_, v)| v.unsigned_abs() > MAX_COEFF.unsigned_abs())
            {
                return Err(format!(
                    "{name} b[{row}] = {v} over the +/-{MAX_COEFF} ceiling"
                ));
            }
            width = l.w.len();
        }
        if width != ACTION_COUNT {
            return Err("head does not produce one logit per action".into());
        }
        let digest = chassis::hash::state_hash(&DigestView {
            gym_version: dto.gym_version,
            q_bits: dto.q_bits,
            features: dto.features,
            conditioning: dto.conditioning,
            actions: dto.actions,
            recips: &dto.recips,
            tanh_lut: &dto.tanh_lut,
            layers: &dto.layers,
            head: &dto.head,
        });
        Ok(Self {
            conditioning: dto.conditioning,
            recips: dto.recips,
            tanh_lut: dto.tanh_lut,
            layers: dto.layers.into_iter().map(|l| (l.w, l.b)).collect(),
            head: (dto.head.w, dto.head.b),
            digest,
        })
    }

    /// This artifact's provenance fingerprint — the number balance
    /// evidence quotes to say which weights produced it. Stable across
    /// reformatting and metadata edits, moved by any coefficient.
    pub fn digest(&self) -> u64 {
        self.digest
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

    /// Number of conditioning knobs this artifact expects.
    pub fn conditioning(&self) -> usize {
        self.conditioning
    }

    /// Q12 logits for gym features plus conditioning knobs (in 0..=1000;
    /// empty for unconditioned artifacts).
    pub fn logits(&self, features: &[i64; FEATURE_COUNT], knobs: &[i64]) -> Vec<i64> {
        // The two saturations that close the kernel's overflow class:
        // an observation is a number the sim computed, not one the
        // artifact's contract covers, so it is held inside the
        // envelope the accumulator bound was derived for. Neither can
        // bind on a reachable observation — MAX_ACT is 262144.0 once
        // normalized, five orders past any feature the gym reports.
        let mut act: Vec<i64> = features
            .iter()
            .chain(knobs.iter().take(self.conditioning))
            .zip(&self.recips)
            .map(|(&f, r)| ((f.clamp(-MAX_FEATURE, MAX_FEATURE) * r) >> Q).clamp(-MAX_ACT, MAX_ACT))
            .collect();
        act.resize(self.recips.len(), 0);
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
    pub fn act(&self, decision: &Decision, knobs: &[i64]) -> Action {
        let logits = self.logits(&decision.features, knobs);
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
/// probability `blunder` (per mille) the commander HESITATES — the
/// decision window passes unused instead of executing the network's
/// pick. The mistakes stay human-shaped — the bot still builds,
/// defends, and attacks, it just dithers — and the dial is
/// continuous, which is what a dynamic difficulty needs. (Blunders
/// once substituted a near-best wrong action instead; that kept
/// spending the Fabricator fund mid-save, so every degraded level
/// rationally collapsed into spam.) Fair by construction: it changes
/// thinking, never income or vision.
#[derive(Debug, Clone)]
pub struct NeuralBot {
    gym: GymBot,
    net: QuantNet,
    knobs: Vec<i64>,
    blunder_permille: u32,
    rng: Pcg32,
}

impl NeuralBot {
    /// A full-strength neural bot for `player` deciding every `cadence`
    /// ticks (use the cadence the network trained at).
    pub fn new(player: PlayerId, cadence: u64, net: QuantNet, faction: Faction) -> Self {
        Self::with_profile(player, cadence, net, 1000, 500, faction, 0, 0)
    }

    /// A profiled bot: `skill` and `aggression` in 0..=1000 plus the
    /// seat's faction feed the network's conditioning inputs (extras are
    /// truncated for artifacts with fewer knobs); skill also derives the
    /// forced-blunder rate unless `blunder_permille` overrides it
    /// (nonzero). The rng is seeded from the scenario like every other
    /// bot — replays hold.
    #[allow(clippy::too_many_arguments)]
    pub fn with_profile(
        player: PlayerId,
        cadence: u64,
        net: QuantNet,
        skill: u32,
        aggression: u32,
        faction: Faction,
        blunder_permille: u32,
        scenario_seed: u64,
    ) -> Self {
        let skill = skill.min(1000);
        let derived = (1000 - skill) / 2; // matches the training mapping
        let blunder = if blunder_permille > 0 {
            blunder_permille.min(1000)
        } else {
            derived
        };
        let faction_knob = match faction {
            Faction::Ferrous => 0,
            Faction::Cupric => 1000,
        };
        Self {
            gym: GymBot::with_cadence(player, cadence),
            net,
            knobs: vec![
                i64::from(skill),
                i64::from(aggression.min(1000)),
                faction_knob,
            ],
            blunder_permille: blunder,
            rng: Pcg32::new(scenario_seed, 3000 + u64::from(player.0)),
        }
    }

    /// The shipped ladder bot: embedded weights, a named difficulty,
    /// and a personality — `aggression` in 0..=1000, or `None` to let
    /// the scenario seed pick one (deterministically: same seed, same
    /// personality).
    pub fn ladder(
        player: PlayerId,
        scenario_seed: u64,
        level: Level,
        aggression: Option<u32>,
        faction: Faction,
    ) -> Self {
        let aggression = aggression.unwrap_or_else(|| deal_aggression(scenario_seed, player));
        Self::with_profile(
            player,
            level.cadence(),
            QuantNet::ladder().clone(),
            level.skill(),
            aggression,
            faction,
            0,
            scenario_seed,
        )
    }

    /// Back-compat constructor: an explicit blunder dial, straight knobs.
    pub fn with_blunder(
        player: PlayerId,
        cadence: u64,
        net: QuantNet,
        blunder_permille: u32,
        scenario_seed: u64,
    ) -> Self {
        Self::with_profile(
            player,
            cadence,
            net,
            1000,
            500,
            Faction::Ferrous,
            blunder_permille,
            scenario_seed,
        )
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
        let mut action = self.net.act(&decision, &self.knobs);
        if self.blunder_permille > 0 && self.rng.next_below(1000) < self.blunder_permille {
            // A blunder is HESITATION — the commander lets a decision
            // window pass — not a wrong action. The 0.10 campaign
            // proved the distinction is structural: second-best-idea
            // blunders kept spending the Fabricator fund mid-save
            // (when the best idea is teching, the runner-up is another
            // sentinel), so every degraded level rationally collapsed
            // to spam and the ladder's lower rungs never showed the
            // varied game. Idling loses tempo — a real handicap that
            // still orders the ladder — without structurally
            // forbidding long plays.
            action = Action::Idle;
        }
        self.gym.step(state, action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The numeric contract's arithmetic, checked rather than argued:
    /// if a future ceiling rises past what `i64` holds, this fails
    /// instead of the kernel wrapping on some artifact nobody tried.
    #[test]
    fn the_accepted_ceilings_cannot_overflow_an_i64_accumulator() {
        MAX_FEATURE
            .checked_mul(MAX_RECIP)
            .expect("scaling a saturated feature by the largest recip");

        let term = i64::from(MAX_COEFF)
            .checked_mul(MAX_ACT)
            .expect("one affine term at both ceilings");
        let row = term
            .checked_mul(MAX_WIDTH as i64)
            .expect("a full-width affine row at both ceilings");
        assert!(
            row.checked_add(i64::from(MAX_COEFF)).is_some(),
            "the bias rides on top of the widest row"
        );

        // Past the first layer an activation is a tanh output, so its
        // ceiling is the table's, not the trunk input's.
        let interpolated = (i64::from(MAX_LUT) - i64::from(-MAX_LUT))
            .checked_mul(127)
            .expect("the tanh interpolation at the table's ceiling");
        assert!(i64::from(MAX_LUT) + (interpolated >> 7) < MAX_ACT);
    }
}
