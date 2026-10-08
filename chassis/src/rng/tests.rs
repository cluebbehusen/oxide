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
    let mut restored: Pcg32 = serde_json::from_str(&serde_json::to_string(&rng).unwrap()).unwrap();
    for _ in 0..100 {
        assert_eq!(rng.next_u32(), restored.next_u32());
    }
}

#[test]
fn deserialization_rejects_even_stream_increments() {
    for inc in [0, 2, u64::MAX - 1] {
        let encoded = serde_json::json!({ "state": 0, "inc": inc });
        let err = serde_json::from_value::<Pcg32>(encoded).unwrap_err();
        assert!(err.to_string().contains("PCG stream increment must be odd"));
    }
    for inc in [1, 3, u64::MAX] {
        for state in [0, u64::MAX] {
            let encoded = serde_json::json!({ "state": state, "inc": inc });
            let rng: Pcg32 = serde_json::from_value(encoded.clone()).unwrap();
            assert_eq!(serde_json::to_value(rng).unwrap(), encoded);
        }
    }
}
