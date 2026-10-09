//! Canonical state hashing.
//!
//! A state hash is the sim's fingerprint: replays assert "tick N hashes to
//! H", and two runs that disagree have desynced. The hash must be stable
//! across platforms and releases, so it is FNV-1a 64 over `postcard`'s
//! canonical byte encoding, never `std::hash` (unstable across Rust
//! versions).

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
/// Panics if `postcard` rejects the value, which derived `Serialize` impls
/// on plain data never do.
pub fn state_hash<T: Serialize + ?Sized>(value: &T) -> u64 {
    let bytes = postcard::to_allocvec(value).expect("sim state must be postcard-serializable");
    fnv1a(&bytes)
}

#[cfg(test)]
mod tests;
