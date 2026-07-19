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

const MULTIPLIER: u64 = 6364136223846793005;

/// A PCG32 generator. Cheap to copy, trivial to serialize.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pcg32 {
    state: u64,
    /// Stream selector (always odd). Two generators with the same seed but
    /// different streams produce unrelated sequences.
    inc: u64,
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
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand_core::RngCore;

    #[test]
    fn matches_pcg_reference_first_output() {
        // First output for seed 42 / stream 54, from the pcg32-demo program
        // in the PCG reference distribution.
        let mut rng = Pcg32::new(42, 54);
        assert_eq!(rng.next_u32(), 0xa15c_02b7);
    }

    #[test]
    fn matches_rand_pcg_across_seeds_and_streams() {
        for (seed, stream) in [(0, 0), (42, 54), (u64::MAX, 7), (0xDEAD_BEEF, u64::MAX)] {
            let mut ours = Pcg32::new(seed, stream);
            let mut reference = rand_pcg::Lcg64Xsh32::new(seed, stream);
            for _ in 0..1000 {
                assert_eq!(ours.next_u32(), reference.next_u32());
            }
        }
    }

    #[test]
    fn next_below_stays_in_bounds_and_hits_all_residues() {
        let mut rng = Pcg32::new(1, 1);
        let mut seen = [false; 7];
        for _ in 0..1000 {
            seen[rng.next_below(7) as usize] = true;
        }
        assert!(seen.iter().all(|&s| s));
    }

    #[test]
    fn serde_roundtrip_continues_the_stream() {
        let mut rng = Pcg32::new(99, 3);
        for _ in 0..17 {
            rng.next_u32();
        }
        let mut restored: Pcg32 =
            serde_json::from_str(&serde_json::to_string(&rng).unwrap()).unwrap();
        for _ in 0..100 {
            assert_eq!(rng.next_u32(), restored.next_u32());
        }
    }
}
