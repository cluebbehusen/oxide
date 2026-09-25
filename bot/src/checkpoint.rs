//! Versioned controller persistence. Query caches are rebuilt; planning progress is retained.
use serde::{Deserialize, Serialize};

/// Wire and controller-continuation revision.
pub const VERSION: u32 = 1;
/// Maximum encoded memory accepted for one controller.
pub const MAX_BYTES: usize = 64 * 1024 * 1024;

/// An opaque, versioned controller checkpoint. Restoration validates its contents
/// against the session before constructing a usable controller.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotCheckpoint {
    pub(crate) version: u32,
    // CBOR preserves compound map keys and the controller's tagged enums.
    #[serde(with = "serde_bytes")]
    pub(crate) payload: Vec<u8>,
}

pub(crate) fn bounded_vec<'de, D, T, const LIMIT: usize>(
    deserializer: D,
) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Bounded<T, const LIMIT: usize>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const LIMIT: usize> serde::de::Visitor<'de> for Bounded<T, LIMIT> {
        type Value = Vec<T>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "at most {LIMIT} entries")
        }

        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            if seq.size_hint().is_some_and(|size| size > LIMIT) {
                return Err(serde::de::Error::custom(format!(
                    "collection exceeds {LIMIT} entries"
                )));
            }
            let mut entries = Vec::new();
            while let Some(entry) = seq.next_element()? {
                if entries.len() == LIMIT {
                    return Err(serde::de::Error::custom(format!(
                        "collection exceeds {LIMIT} entries"
                    )));
                }
                entries.push(entry);
            }
            Ok(entries)
        }
    }
    deserializer.deserialize_seq(Bounded::<T, LIMIT>(std::marker::PhantomData))
}

#[cfg(test)]
pub(crate) fn round_trip<T>(value: &T) -> T
where
    T: Serialize + serde::de::DeserializeOwned + std::fmt::Debug + PartialEq,
{
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes).unwrap();
    let restored = ciborium::from_reader(bytes.as_slice()).unwrap();
    assert_eq!(value, &restored);
    restored
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_payload_uses_a_compact_byte_string() {
        let checkpoint = BotCheckpoint {
            version: VERSION,
            payload: (0..=255).cycle().take(4096).collect(),
        };
        let mut bytes = Vec::new();
        ciborium::into_writer(&checkpoint, &mut bytes).unwrap();
        assert!(
            bytes.len() <= checkpoint.payload.len() + 128,
            "binary payload expanded from {} to {} bytes",
            checkpoint.payload.len(),
            bytes.len()
        );
        let restored: BotCheckpoint = ciborium::from_reader(bytes.as_slice()).unwrap();
        assert_eq!(checkpoint.payload, restored.payload);
        assert_eq!(checkpoint.version, restored.version);
    }
}
