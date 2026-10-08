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
