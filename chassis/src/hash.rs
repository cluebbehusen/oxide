//! Canonical state hashing.
//!
//! A state hash is the sim's fingerprint: replays assert "tick N hashes to
//! H", and any two runs that disagree have desynced. The hash must therefore
//! be stable across platforms and releases — so it is FNV-1a 64 (a frozen,
//! trivial algorithm) over `postcard`'s canonical byte encoding, never
//! `std::hash` (explicitly unstable across Rust versions).

use serde::Serialize;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a 64-bit over raw bytes.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Hashes any serializable value via its canonical `postcard` encoding.
///
/// Panics if serialization fails, which for plain data types (no maps with
/// nondeterministic order, no floats — i.e. anything a sim is allowed to
/// contain) cannot happen.
pub fn state_hash<T: Serialize + ?Sized>(value: &T) -> u64 {
    let bytes = postcard::to_allocvec(value).expect("sim state must be postcard-serializable");
    fnv1a(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_matches_published_vectors() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn state_hash_is_stable_for_equal_values() {
        #[derive(Serialize)]
        struct Demo {
            a: u32,
            b: Vec<i64>,
        }
        let x = Demo {
            a: 7,
            b: vec![-1, 2, 3],
        };
        let y = Demo {
            a: 7,
            b: vec![-1, 2, 3],
        };
        assert_eq!(state_hash(&x), state_hash(&y));
    }

    #[test]
    fn state_hash_distinguishes_different_values() {
        assert_ne!(state_hash(&1u32), state_hash(&2u32));
    }
}
