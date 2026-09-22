//! Versioned controller persistence. Query caches are rebuilt; planning progress is retained.
use serde::{Deserialize, Serialize};

/// Wire and controller-continuation revision. Changes to persisted layouts or
/// their interpretation require a new revision and an explicit migration.
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
    pub(crate) payload: Vec<u8>,
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
