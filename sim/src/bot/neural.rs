//! Deterministic integer inference for a trained policy.
//!
//! The sim may not touch floats, so a shipped network is a fixed-point
//! artifact produced by `tools/train/export.py`: Q12 weights, a Q12
//! tanh lookup table, per-feature reciprocal scales. Everything here is
//! `i64` adds, multiplies, and shifts — bit-identical on every
//! platform, which is what lets a neural policy live inside replays
//! and hash fixtures like any other command source. The quantized
//! network is a slightly different player than the float one that
//! trained (12 fractional bits), so promotion tournaments run against
//! *this* bot, not the torch checkpoint. No artifact ships embedded
//! today: the frozen 0.14 ladder is deleted, and the retrained v9
//! actor loads from a file until promotion embeds it.

use super::gym::{
    ACTION_COUNT, ACTION_HEADS, ActionPlan, Decision, FEATURE_COUNT, GYM_VERSION, GymBot,
};
use super::profile::{PROFILE_CONDITION_NAMES, ProfileFacets, ResolvedBotProfile};
use crate::command::PlayerCommand;
use crate::ids::PlayerId;
use crate::state::Faction;
use crate::state::State;
use chassis::rng::Pcg32;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::fmt::Write as _;

/// Stream selector a seat's hesitation rng runs on: this base plus the
/// player index. Every shipped path uses that derivation; the constant
/// is public so a fairness probe can exchange the streams between seats
/// and measure whether the seat-bound assignment carries an advantage.
pub const DECISION_STREAM_BASE: u64 = 3000;

/// Skill, aggression, faction, a four-way strategy one-hot, and five
/// resolved strategic profile facets.
pub const CONDITIONING_COUNT: usize = 12;

/// One stable name per policy-conditioning column.
///
/// The gym hello publishes this exact table alongside Rust-authored named
/// profile values, so training code never reconstructs a named vector from
/// duplicated constants.
pub const CONDITION_NAMES: [&str; CONDITIONING_COUNT] = [
    "skill",
    "aggression",
    "faction",
    "strategy_fortify",
    "strategy_industry",
    "strategy_combined",
    "strategy_pressure",
    PROFILE_CONDITION_NAMES[0],
    PROFILE_CONDITION_NAMES[1],
    PROFILE_CONDITION_NAMES[2],
    PROFILE_CONDITION_NAMES[3],
    PROFILE_CONDITION_NAMES[4],
];

/// Floor of the seed-dealt personality range. An explicit `aggression`
/// pick may use the full 0..=1000 conditioning the network trained
/// under; the deal narrows it to two measured-safe styles. Industry
/// profiles supply the Reclaimer-heavy economic variety that the
/// combined-arms profile lacks, while the pressure quartile has a sharp
/// failure at its 750 boundary. The extremes stay reachable through
/// explicit picks for experiments and custom matches.
pub const DEALT_AGGRESSION_MIN: u32 = 250;
/// Ceiling of the seed-dealt personality range. The deal skips the
/// unmeasured transition between its two style bands and every profile
/// above this point; the first measured inactive tail appeared at 625.
pub const DEALT_AGGRESSION_MAX: u32 = 600;

const DEALT_INDUSTRY_MAX: u32 = 399;
const DEALT_COMBINED_MIN: u32 = 500;

/// Deals the legacy raw personality used by compatibility constructors
/// and dial-isolation diagnostics: three chances in five of an industry
/// band and two in five of combined arms, deterministic from the scenario
/// seed. Ordinary named-profile scenarios resolve style, variant, and
/// facets through [`super::profile::resolve_bot_profiles`] instead.
pub fn deal_aggression(scenario_seed: u64, player: PlayerId) -> u32 {
    let mut rng = Pcg32::new(scenario_seed, 4000 + u64::from(player.0));
    if rng.next_below(5) < 3 {
        DEALT_AGGRESSION_MIN + rng.next_below(DEALT_INDUSTRY_MAX - DEALT_AGGRESSION_MIN + 1)
    } else {
        DEALT_COMBINED_MIN + rng.next_below(DEALT_AGGRESSION_MAX - DEALT_COMBINED_MIN + 1)
    }
}

/// The player-facing difficulty ladder: one trained network, four
/// execution handicaps. Difficulty changes hesitation and reaction
/// cadence, never rules or information. The enum is authored scenario
/// data (`BotConfig.level`) and survives the 0.14 actor's deletion;
/// the retrained actor re-measures every handicap before promotion.
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

    /// The historical raw skill profile for dial-isolation experiments.
    /// Named ladder play does not feed this value to the network:
    /// learned skill conditioning is not monotonic, so ladder
    /// constructors use a measured strategy-specific condition and take
    /// only hesitation and cadence from the level.
    pub fn skill(self) -> u32 {
        match self {
            Level::Easy => 300,
            Level::Medium => 620,
            Level::Hard => 850,
            Level::Expert => 1000,
        }
    }

    /// Decision windows skipped by this level, per mille. Unlike the raw
    /// profile API's legacy zero sentinel, this is an exact rate: Expert
    /// therefore represents zero hesitation rather than deriving it from
    /// the policy-conditioning skill.
    ///
    /// Re-measured for the 0.15 actor with the ladder handicap sweep:
    /// the retrained policy holds 33-40/40 on the Overseer yardstick
    /// everywhere below 650 per mille (the 0.14 rungs at 350/190/5 all
    /// saturated), and its strength falls off a cliff between 800 and
    /// 900. Hesitation is the ladder's real lever; the rungs sit at
    /// 15/28/36/40 wins with strictly falling tick totals.
    pub fn hesitation_permille(self) -> u32 {
        match self {
            Level::Easy => 900,
            Level::Medium => 800,
            Level::Hard => 650,
            Level::Expert => 0,
        }
    }

    /// How often this level thinks, in ticks — the second execution
    /// handicap. Measured for the 0.15 actor: the cadence response
    /// saturates past 64 ticks (identical slates), and 34 is both the
    /// strongest and fastest-finishing full-strength point (the 0.14
    /// actor's 37 is slower without being safer). Medium alone thinks
    /// at 48 — under its 800-per-mille hesitation the slower clock
    /// separates it from Hard by 8 wins instead of 3.
    pub fn cadence(self) -> u64 {
        match self {
            Level::Easy => 34,
            Level::Medium => 48,
            Level::Hard => 34,
            Level::Expert => 34,
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
    /// Named conditioning knobs appended after the gym features.
    #[serde(default)]
    conditioning: usize,
    actions: usize,
    recips: Vec<i64>,
    tanh_lut: Vec<i32>,
    layers: Vec<LayerDto>,
    head: LayerDto,
    #[serde(default)]
    lineage: Option<serde_json::Value>,
}

const LINEAGE_SCHEMA: u64 = 1;
const SHA256_PREFIX: &str = "sha256:";

fn is_sha256(value: &str) -> bool {
    value.strip_prefix(SHA256_PREFIX).is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn write_python_json_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{0008}' => output.push_str("\\b"),
            '\u{000c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            ' '..='~' => output.push(character),
            character if u32::from(character) <= 0xffff => {
                write!(output, "\\u{:04x}", u32::from(character))
                    .expect("writing into a String cannot fail");
            }
            character => {
                let scalar = u32::from(character) - 0x1_0000;
                let high = 0xd800 + (scalar >> 10);
                let low = 0xdc00 + (scalar & 0x3ff);
                write!(output, "\\u{high:04x}\\u{low:04x}")
                    .expect("writing into a String cannot fail");
            }
        }
    }
    output.push('"');
}

fn write_python_json_number(output: &mut String, number: &serde_json::Number) {
    let rendered = number.to_string();
    if let Some(exponent_at) = rendered.find(['e', 'E']) {
        let (mantissa, exponent) = rendered.split_at(exponent_at);
        let exponent = exponent[1..]
            .parse::<i32>()
            .expect("serde_json rendered a valid exponent");
        output.push_str(mantissa);
        write!(output, "e{exponent:+03}").expect("writing into a String cannot fail");
        return;
    }

    // serde_json's formatter deliberately keeps decimal exponent -5 in
    // fixed notation; CPython's repr, which lineage.py hashes, switches
    // to scientific notation below -4. The upper fixed boundary (15)
    // already agrees.
    let (sign, unsigned) = rendered
        .strip_prefix('-')
        .map_or(("", rendered.as_str()), |unsigned| ("-", unsigned));
    if let Some(fraction) = unsigned.strip_prefix("0.")
        && let Some(first_nonzero) = fraction.bytes().position(|byte| byte != b'0')
        && first_nonzero == 4
    {
        let significant = &fraction[first_nonzero..];
        output.push_str(sign);
        output.push(char::from(significant.as_bytes()[0]));
        if significant.len() > 1 {
            output.push('.');
            output.push_str(&significant[1..]);
        }
        output.push_str("e-05");
        return;
    }
    output.push_str(&rendered);
}

fn write_python_canonical_json(output: &mut String, value: &serde_json::Value) {
    match value {
        serde_json::Value::Null => output.push_str("null"),
        serde_json::Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        serde_json::Value::Number(number) => write_python_json_number(output, number),
        serde_json::Value::String(value) => write_python_json_string(output, value),
        serde_json::Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                write_python_canonical_json(output, value);
            }
            output.push(']');
        }
        serde_json::Value::Object(values) => {
            output.push('{');
            let mut entries: Vec<_> = values.iter().collect();
            entries.sort_unstable_by_key(|(key, _)| *key);
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                write_python_json_string(output, key);
                output.push(':');
                write_python_canonical_json(output, value);
            }
            output.push('}');
        }
    }
}

fn validate_lineage(value: &serde_json::Value) -> Result<(), String> {
    let manifest = value
        .as_object()
        .ok_or_else(|| "lineage metadata must be an object".to_string())?;
    let lineage_id = manifest
        .get("lineage_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "lineage metadata must carry a lineage_id".to_string())?;
    if !is_sha256(lineage_id) {
        return Err("lineage_id must be a SHA-256 digest".into());
    }
    if manifest.get("schema").and_then(serde_json::Value::as_u64) != Some(LINEAGE_SCHEMA) {
        return Err(format!(
            "unsupported lineage schema {:?}; expected {LINEAGE_SCHEMA}",
            manifest.get("schema")
        ));
    }
    if !manifest
        .get("phase")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|phase| !phase.is_empty())
    {
        return Err("lineage phase must be a non-empty string".into());
    }
    if manifest
        .get("phase_start_update")
        .and_then(serde_json::Value::as_u64)
        .is_none()
    {
        return Err("lineage phase_start_update must be a non-negative integer".into());
    }
    if !manifest
        .get("hyperparameters")
        .is_some_and(serde_json::Value::is_object)
    {
        return Err("lineage hyperparameters must be an object".into());
    }
    let inputs = manifest
        .get("inputs")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "lineage inputs must be an object".to_string())?;
    for (role, identity) in inputs {
        if role.is_empty() {
            return Err("lineage input roles must be non-empty strings".into());
        }
        let identity = identity
            .as_object()
            .ok_or_else(|| format!("lineage input {role:?} must be an object"))?;
        if !identity
            .get("content_sha256")
            .and_then(serde_json::Value::as_str)
            .is_some_and(is_sha256)
        {
            return Err(format!(
                "lineage input {role:?} must carry a SHA-256 content digest"
            ));
        }
        if identity
            .get("lineage_id")
            .is_some_and(|upstream| !upstream.as_str().is_some_and(is_sha256))
        {
            return Err(format!(
                "lineage input {role:?} carries an invalid upstream lineage id"
            ));
        }
    }

    let mut payload = manifest.clone();
    payload.remove("lineage_id");
    let mut canonical = String::new();
    write_python_canonical_json(&mut canonical, &serde_json::Value::Object(payload));
    let digest = Sha256::digest(canonical.as_bytes());
    let mut expected = String::from(SHA256_PREFIX);
    for byte in digest {
        write!(expected, "{byte:02x}").expect("writing into a String cannot fail");
    }
    if expected != lineage_id {
        return Err("lineage_id does not match the lineage manifest".into());
    }
    Ok(())
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
    /// Parses an exported artifact, refusing version, shape, or
    /// magnitude drift. The magnitude ceilings are what make the
    /// integer kernel total: see the numeric contract above.
    pub fn from_json(json: &str) -> Result<Self, String> {
        let dto: ArtifactDto = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if let Some(lineage) = &dto.lineage {
            validate_lineage(lineage)?;
        }
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
        if dto.conditioning != CONDITIONING_COUNT {
            return Err(format!(
                "conditioning is {} wide, gym v{GYM_VERSION} requires {CONDITIONING_COUNT}",
                dto.conditioning
            ));
        }
        // The width ceiling must gate the raw scalar BEFORE the add: a
        // hostile `conditioning` near usize::MAX would otherwise
        // overflow the sum itself — a debug panic and a release wrap,
        // the exact build-profile split this boundary exists to refuse.
        if dto.conditioning > MAX_WIDTH {
            return Err(format!(
                "conditioning is {} wide, over the {MAX_WIDTH} ceiling",
                dto.conditioning
            ));
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

    /// Greedy masked action plan: one highest-logit legal choice per
    /// head, ties to the first action in that head's declared order.
    pub fn act(&self, decision: &Decision, knobs: &[i64]) -> ActionPlan {
        let logits = self.logits(&decision.features, knobs);
        let mut choices = ActionPlan::default().indices();
        for (head_index, head) in ACTION_HEADS.iter().enumerate() {
            let mut best: Option<(i64, usize)> = None;
            for &action in *head {
                if decision.mask[action] && best.is_none_or(|(value, _)| logits[action] > value) {
                    best = Some((logits[action], action));
                }
            }
            if let Some((_, action)) = best {
                choices[head_index] = action;
            }
        }
        ActionPlan::from_indices(choices)
    }
}

fn profile_knobs(
    skill: u32,
    aggression: u32,
    faction: Faction,
    facets: ProfileFacets,
) -> [i64; CONDITIONING_COUNT] {
    let strategy = match aggression {
        0..=249 => 0,
        250..=499 => 1,
        500..=749 => 2,
        _ => 3,
    };
    let [economy, air, siege, support, commitment] = facets.conditions();
    [
        i64::from(skill),
        i64::from(aggression),
        i64::from(faction == Faction::Cupric) * 1000,
        i64::from(strategy == 0) * 1000,
        i64::from(strategy == 1) * 1000,
        i64::from(strategy == 2) * 1000,
        i64::from(strategy == 3) * 1000,
        i64::from(economy),
        i64::from(air),
        i64::from(siege),
        i64::from(support),
        i64::from(commitment),
    ]
}

fn ladder_policy_skill(aggression: u32) -> u32 {
    match aggression {
        250..=499 => 620,
        _ => 1000,
    }
}

/// The conditioning values for a raw-aggression ladder profile.
///
/// Named difficulty is intentionally absent: ladder levels change only
/// cadence and hesitation. Raw profile tools deliberately append zero
/// facets rather than inventing a named strategic lean.
pub fn ladder_condition_values(aggression: u32, faction: Faction) -> [i64; CONDITIONING_COUNT] {
    let aggression = aggression.min(1000);
    profile_knobs(
        ladder_policy_skill(aggression),
        aggression,
        faction,
        ProfileFacets::ZERO,
    )
}

/// The complete condition for one resolved named profile.
pub fn ladder_condition_values_with_facets(
    aggression: u32,
    faction: Faction,
    facets: ProfileFacets,
) -> [i64; CONDITIONING_COUNT] {
    let aggression = aggression.min(1000);
    profile_knobs(ladder_policy_skill(aggression), aggression, faction, facets)
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
    /// (nonzero). This preserves the raw experimental API's legacy zero
    /// sentinel; [`Self::with_profile_hesitation`] accepts an exact zero.
    /// The rng is seeded from the scenario like every other bot — replays
    /// hold.
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
        Self::with_profile_stream(
            player,
            cadence,
            net,
            skill,
            aggression,
            faction,
            blunder_permille,
            scenario_seed,
            DECISION_STREAM_BASE + u64::from(player.0),
        )
    }

    /// A raw experimental profile with an explicit hesitation override.
    /// `None` derives hesitation from `skill`; `Some(0)` means no
    /// hesitation. Named ladder and candidate-gate paths use this exact
    /// form so their execution handicap is independent of policy
    /// conditioning.
    #[allow(clippy::too_many_arguments)]
    pub fn with_profile_hesitation(
        player: PlayerId,
        cadence: u64,
        net: QuantNet,
        skill: u32,
        aggression: u32,
        faction: Faction,
        hesitation_permille: Option<u32>,
        scenario_seed: u64,
    ) -> Self {
        Self::profile(
            player,
            cadence,
            net,
            skill,
            aggression,
            faction,
            ProfileFacets::ZERO,
            hesitation_permille,
            scenario_seed,
            DECISION_STREAM_BASE + u64::from(player.0),
        )
    }

    /// [`Self::with_profile`] with the hesitation rng's stream selector
    /// named outright instead of derived from the seat. Every shipped
    /// path takes the derived stream; an explicit one exists so a
    /// fairness probe can exchange two seats' streams and leave
    /// everything else alone.
    #[allow(clippy::too_many_arguments)]
    pub fn with_profile_stream(
        player: PlayerId,
        cadence: u64,
        net: QuantNet,
        skill: u32,
        aggression: u32,
        faction: Faction,
        blunder_permille: u32,
        scenario_seed: u64,
        stream: u64,
    ) -> Self {
        Self::profile(
            player,
            cadence,
            net,
            skill,
            aggression,
            faction,
            ProfileFacets::ZERO,
            (blunder_permille > 0).then_some(blunder_permille),
            scenario_seed,
            stream,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn profile(
        player: PlayerId,
        cadence: u64,
        net: QuantNet,
        skill: u32,
        aggression: u32,
        faction: Faction,
        facets: ProfileFacets,
        hesitation_permille: Option<u32>,
        scenario_seed: u64,
        stream: u64,
    ) -> Self {
        let skill = skill.min(1000);
        let derived = (1000 - skill) / 2; // matches the training mapping
        let blunder = hesitation_permille.unwrap_or(derived).min(1000);
        let aggression = aggression.min(1000);
        Self {
            gym: GymBot::with_profile_facets(player, cadence, facets),
            net,
            knobs: profile_knobs(skill, aggression, faction, facets).to_vec(),
            blunder_permille: blunder,
            rng: Pcg32::new(scenario_seed, stream),
        }
    }

    /// Applies one resolved named profile to an arbitrary candidate actor.
    ///
    /// Promotion diagnostics use this path to measure the exact runtime
    /// wrapper without reconstructing profile facets outside the sim.
    pub fn ladder_resolved_with_net(
        player: PlayerId,
        scenario_seed: u64,
        profile: ResolvedBotProfile,
        faction: Faction,
        net: QuantNet,
    ) -> Self {
        Self::ladder_resolved_with_net_at_cadence(
            player,
            scenario_seed,
            profile,
            faction,
            net,
            profile.level.cadence(),
        )
    }

    /// Applies a resolved named profile while selecting the hesitation rng
    /// stream explicitly.
    ///
    /// Ordinary play derives the stream from the seat. The factorial fairness
    /// probe uses this form to exchange only that seat-bound stream while
    /// preserving the complete style, variant, and team-role condition.
    pub fn ladder_resolved_with_net_at_stream(
        player: PlayerId,
        scenario_seed: u64,
        profile: ResolvedBotProfile,
        faction: Faction,
        net: QuantNet,
        stream: u64,
    ) -> Self {
        Self::profile(
            player,
            profile.level.cadence(),
            net,
            ladder_policy_skill(profile.aggression),
            profile.aggression,
            faction,
            profile.facets,
            Some(profile.level.hesitation_permille()),
            scenario_seed,
            stream,
        )
    }

    /// Applies a resolved profile to a candidate with only cadence
    /// overridden.
    ///
    /// This is the named-profile counterpart to
    /// [`Self::ladder_with_net_at_cadence`]: promotion probes can
    /// isolate decision frequency without dropping the profile facets
    /// the candidate will receive after deployment.
    pub fn ladder_resolved_with_net_at_cadence(
        player: PlayerId,
        scenario_seed: u64,
        profile: ResolvedBotProfile,
        faction: Faction,
        net: QuantNet,
        cadence: u64,
    ) -> Self {
        Self::profile(
            player,
            cadence,
            net,
            ladder_policy_skill(profile.aggression),
            profile.aggression,
            faction,
            profile.facets,
            Some(profile.level.hesitation_permille()),
            scenario_seed,
            DECISION_STREAM_BASE + u64::from(player.0),
        )
    }

    /// Applies the backward-compatible raw zero-facet ladder wrapper to
    /// an arbitrary quantized network.
    pub fn ladder_with_net(
        player: PlayerId,
        scenario_seed: u64,
        level: Level,
        aggression: Option<u32>,
        faction: Faction,
        net: QuantNet,
    ) -> Self {
        Self::ladder_with_net_at_cadence(
            player,
            scenario_seed,
            level,
            aggression,
            faction,
            net,
            level.cadence(),
        )
    }

    /// Applies the raw zero-facet ladder profile while selecting the
    /// hesitation rng stream explicitly.
    #[allow(clippy::too_many_arguments)]
    pub fn ladder_with_net_at_stream(
        player: PlayerId,
        scenario_seed: u64,
        level: Level,
        aggression: Option<u32>,
        faction: Faction,
        net: QuantNet,
        stream: u64,
    ) -> Self {
        Self::ladder_with_net_at_cadence_and_stream(
            player,
            scenario_seed,
            level,
            aggression,
            faction,
            net,
            level.cadence(),
            stream,
        )
    }

    /// The raw zero-facet ladder wrapper with only its decision cadence
    /// overridden.
    #[allow(clippy::too_many_arguments)]
    pub fn ladder_with_net_at_cadence(
        player: PlayerId,
        scenario_seed: u64,
        level: Level,
        aggression: Option<u32>,
        faction: Faction,
        net: QuantNet,
        cadence: u64,
    ) -> Self {
        Self::ladder_with_net_at_cadence_and_stream(
            player,
            scenario_seed,
            level,
            aggression,
            faction,
            net,
            cadence,
            DECISION_STREAM_BASE + u64::from(player.0),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn ladder_with_net_at_cadence_and_stream(
        player: PlayerId,
        scenario_seed: u64,
        level: Level,
        aggression: Option<u32>,
        faction: Faction,
        net: QuantNet,
        cadence: u64,
        stream: u64,
    ) -> Self {
        let aggression = aggression.unwrap_or_else(|| deal_aggression(scenario_seed, player));
        Self::profile(
            player,
            cadence,
            net,
            ladder_policy_skill(aggression),
            aggression,
            faction,
            ProfileFacets::ZERO,
            Some(level.hesitation_permille()),
            scenario_seed,
            stream,
        )
    }

    /// The raw zero-facet ladder wrapper with both execution handicaps
    /// explicit. Promotion's handicap-recalibration sweep measures a
    /// candidate across the (hesitation, cadence) plane under exactly
    /// the shipped ladder conditioning; [`Level`] then pins the
    /// measured rungs.
    pub fn ladder_with_net_at_execution(
        player: PlayerId,
        scenario_seed: u64,
        aggression: Option<u32>,
        faction: Faction,
        net: QuantNet,
        hesitation_permille: u32,
        cadence: u64,
    ) -> Self {
        let aggression = aggression.unwrap_or_else(|| deal_aggression(scenario_seed, player));
        Self::profile(
            player,
            cadence,
            net,
            ladder_policy_skill(aggression),
            aggression,
            faction,
            ProfileFacets::ZERO,
            Some(hesitation_permille),
            scenario_seed,
            DECISION_STREAM_BASE + u64::from(player.0),
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
        let mut plan = self.net.act(&decision, &self.knobs);
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
            plan = ActionPlan::default();
        }
        self.gym.step_plan(state, plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::canonical_profiles;

    /// A hand-built v9 artifact: identity-ish single layer. The
    /// constructor contracts under test read knobs, cadence, and rng —
    /// any shape-valid net serves them.
    fn tiny_net() -> QuantNet {
        let inputs = FEATURE_COUNT + CONDITIONING_COUNT;
        let lut: Vec<i32> = (0..=512).map(|i| (i - 256) * 16).collect();
        let trunk_w: Vec<Vec<i32>> = (0..4)
            .map(|o| {
                let mut row = vec![0; inputs];
                row[o] = 4096;
                row
            })
            .collect();
        let head_w: Vec<Vec<i32>> = (0..ACTION_COUNT)
            .map(|o| {
                let mut row = vec![0; 4];
                if o < 4 {
                    row[o] = 4096;
                }
                row
            })
            .collect();
        let artifact = serde_json::json!({
            "gym_version": GYM_VERSION,
            "q_bits": 12,
            "features": FEATURE_COUNT,
            "conditioning": CONDITIONING_COUNT,
            "actions": ACTION_COUNT,
            "recips": vec![1i64 << 24; inputs],
            "tanh_lut": lut,
            "layers": [{"w": trunk_w, "b": vec![0; 4]}],
            "head": {"w": head_w, "b": vec![0; ACTION_COUNT]},
        });
        QuantNet::from_json(&artifact.to_string()).expect("the tiny artifact is shape-valid")
    }

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

    #[test]
    fn strategy_conditioning_uses_deterministic_aggression_quartiles() {
        for (aggression, strategy) in [
            (0, 0),
            (249, 0),
            (250, 1),
            (499, 1),
            (500, 2),
            (749, 2),
            (750, 3),
            (1000, 3),
        ] {
            let knobs = profile_knobs(800, aggression, Faction::Cupric, ProfileFacets::ZERO);
            assert_eq!(knobs.len(), CONDITIONING_COUNT);
            assert_eq!(&knobs[..3], &[800, i64::from(aggression), 1000]);
            assert_eq!(
                &knobs[3..7],
                &(0..4)
                    .map(|index| if index == strategy { 1000 } else { 0 })
                    .collect::<Vec<_>>()
            );
            assert_eq!(&knobs[7..], &[0; 5]);
        }
    }

    #[test]
    fn legacy_raw_deal_stays_inside_the_promoted_safe_window() {
        let mut saw_industry = false;
        let mut saw_combined = false;
        let mut industry_count = 0;
        for seed in 0..10_000 {
            let aggression = deal_aggression(seed, PlayerId((seed % 2) as u8));
            assert!((DEALT_AGGRESSION_MIN..=DEALT_AGGRESSION_MAX).contains(&aggression));
            assert!(
                aggression <= DEALT_INDUSTRY_MAX || aggression >= DEALT_COMBINED_MIN,
                "the unsupported transition band must never be dealt"
            );
            saw_industry |= aggression <= DEALT_INDUSTRY_MAX;
            saw_combined |= aggression >= DEALT_COMBINED_MIN;
            industry_count += u32::from(aggression <= DEALT_INDUSTRY_MAX);
        }
        assert!(
            saw_industry && saw_combined,
            "the deterministic deal reaches both promoted styles"
        );
        assert!(
            (5_700..=6_300).contains(&industry_count),
            "the deal remains close to its three-in-five industry weight: {industry_count}/10000"
        );
    }

    #[test]
    fn raw_ladder_levels_use_strategy_skill_and_level_execution_handicaps() {
        for level in Level::LADDER {
            for (aggression, policy_skill) in [(300, 620), (550, 1000)] {
                let raw = NeuralBot::ladder_with_net(
                    PlayerId(0),
                    17,
                    level,
                    Some(aggression),
                    Faction::Ferrous,
                    tiny_net(),
                );
                assert_eq!(raw.knobs[0], policy_skill);
                assert_eq!(raw.blunder_permille, level.hesitation_permille());
                assert_eq!(raw.gym.cadence(), level.cadence());

                let explicit_stream = NeuralBot::ladder_with_net_at_stream(
                    PlayerId(0),
                    17,
                    level,
                    Some(aggression),
                    Faction::Ferrous,
                    tiny_net(),
                    DECISION_STREAM_BASE,
                );
                assert_eq!(explicit_stream.knobs, raw.knobs);
                assert_eq!(explicit_stream.blunder_permille, raw.blunder_permille);
                assert_eq!(explicit_stream.gym.cadence(), raw.gym.cadence());
                assert_eq!(explicit_stream.rng, raw.rng);
            }
        }
    }

    #[test]
    fn resolved_profile_cadence_override_preserves_every_policy_knob() {
        let canonical = canonical_profiles()[4];
        let profile = ResolvedBotProfile {
            level: Level::Medium,
            style: Some(canonical.style),
            variant: Some(canonical.variant),
            aggression: canonical.aggression,
            team_role: crate::scenario::TeamRole::Support,
            facets: canonical
                .facets
                .with_role(crate::scenario::TeamRole::Support),
        };
        let named = NeuralBot::ladder_resolved_with_net(
            PlayerId(0),
            17,
            profile,
            Faction::Cupric,
            tiny_net(),
        );
        let faster = NeuralBot::ladder_resolved_with_net_at_cadence(
            PlayerId(0),
            17,
            profile,
            Faction::Cupric,
            tiny_net(),
            11,
        );

        assert_eq!(faster.knobs, named.knobs);
        assert_eq!(faster.blunder_permille, named.blunder_permille);
        assert_eq!(faster.rng, named.rng);
        assert_eq!(faster.gym.cadence(), 11);
    }

    #[test]
    fn raw_ladder_stream_override_changes_only_the_hesitation_stream() {
        let raw = NeuralBot::ladder_with_net(
            PlayerId(0),
            17,
            Level::Medium,
            Some(550),
            Faction::Ferrous,
            tiny_net(),
        );
        let crossed = NeuralBot::ladder_with_net_at_stream(
            PlayerId(0),
            17,
            Level::Medium,
            Some(550),
            Faction::Ferrous,
            tiny_net(),
            DECISION_STREAM_BASE + 1,
        );
        assert_eq!(crossed.knobs, raw.knobs);
        assert_eq!(crossed.blunder_permille, raw.blunder_permille);
        assert_eq!(crossed.gym.cadence(), raw.gym.cadence());
        assert_ne!(crossed.rng, raw.rng);
    }

    #[test]
    fn resolved_profile_stream_override_changes_only_the_hesitation_stream() {
        let canonical = canonical_profiles()[1];
        let profile = ResolvedBotProfile {
            level: Level::Medium,
            style: Some(canonical.style),
            variant: Some(canonical.variant),
            aggression: canonical.aggression,
            team_role: crate::scenario::TeamRole::Industry,
            facets: canonical
                .facets
                .with_role(crate::scenario::TeamRole::Industry),
        };
        let named = NeuralBot::ladder_resolved_with_net(
            PlayerId(0),
            17,
            profile,
            Faction::Ferrous,
            tiny_net(),
        );
        let crossed = NeuralBot::ladder_resolved_with_net_at_stream(
            PlayerId(0),
            17,
            profile,
            Faction::Ferrous,
            tiny_net(),
            DECISION_STREAM_BASE + 1,
        );
        assert_eq!(crossed.knobs, named.knobs);
        assert_eq!(crossed.blunder_permille, named.blunder_permille);
        assert_eq!(crossed.gym.cadence(), named.gym.cadence());
        assert_ne!(crossed.rng, named.rng);
        assert_ne!(&named.knobs[7..], &[0; 5]);
    }

    #[test]
    fn exact_zero_hesitation_does_not_change_the_legacy_profile_api() {
        let legacy = NeuralBot::with_profile(
            PlayerId(0),
            16,
            tiny_net(),
            620,
            300,
            Faction::Ferrous,
            0,
            17,
        );
        let exact = NeuralBot::with_profile_hesitation(
            PlayerId(0),
            16,
            tiny_net(),
            620,
            300,
            Faction::Ferrous,
            Some(0),
            17,
        );

        assert_eq!(legacy.blunder_permille, 190);
        assert_eq!(exact.blunder_permille, 0);
        assert_eq!(legacy.knobs, exact.knobs);
    }
}
