//! Deterministic random numbers.
//!
//! A hand-rolled PCG32 (XSH-RR 64/32, O'Neill 2014). Hand-rolled not out of
//! pride but for stability: the algorithm is frozen here, in ~30 lines we
//! control, so no dependency upgrade can ever silently change the stream and
//! invalidate every replay. A test cross-checks it against `rand_pcg`'s
//! reference implementation.
//!
//! The generator state is plain serializable data — snapshot a sim mid-run,
//! restore it, and the stream continues bit-for-bit.

use serde::{Deserialize, Serialize};

const MULTIPLIER: u64 = 6_364_136_223_846_793_005;

/// A PCG32 generator. Cheap to copy, trivial to serialize.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pcg32 {
    state: u64,
    /// Stream selector (always odd). Two generators with the same seed but
    /// different streams produce unrelated sequences.
    #[serde(deserialize_with = "deserialize_increment")]
    inc: u64,
}

fn deserialize_increment<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<u64, D::Error> {
    let inc = u64::deserialize(deserializer)?;
    if inc & 1 == 0 {
        return Err(serde::de::Error::custom("PCG stream increment must be odd"));
    }
    Ok(inc)
}

impl Pcg32 {
    /// Creates a generator from a seed and a stream id, per the PCG
    /// reference `pcg32_srandom_r` initialization.
    pub fn new(seed: u64, stream: u64) -> Self {
        let mut rng = Self {
            state: 0,
            inc: (stream << 1) | 1,
        };
        rng.next_u32();
        rng.state = rng.state.wrapping_add(seed);
        rng.next_u32();
        rng
    }

    /// Next 32 uniformly distributed bits.
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(MULTIPLIER).wrapping_add(self.inc);
        let xorshifted = ((((old >> 18) ^ old) >> 27) & 0xFFFF_FFFF) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// Next 64 uniformly distributed bits (two draws, high word first).
    pub fn next_u64(&mut self) -> u64 {
        let hi = u64::from(self.next_u32());
        let lo = u64::from(self.next_u32());
        (hi << 32) | lo
    }

    /// Uniform draw from `0..bound` without modulo bias (rejection
    /// sampling). Panics if `bound` is zero.
    pub fn next_below(&mut self, bound: u32) -> u32 {
        assert!(bound > 0, "next_below requires a nonzero bound");
        let threshold = bound.wrapping_neg() % bound;
        loop {
            let r = self.next_u32();
            if r >= threshold {
                return r % bound;
            }
        }
    }

    /// Uniform index into a collection of `len` items, drawn exactly as
    /// [`Pcg32::next_below`] draws `len`.
    pub fn next_index(&mut self, len: usize) -> usize {
        self.next_below(u32::try_from(len).expect("collection lengths fit in u32")) as usize
    }
}

#[cfg(test)]
mod tests;
