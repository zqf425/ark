use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

/// Newtype ids (design §6.3).
///
/// The wrap exists so "passed thread id where turn id was expected" becomes a
/// compile error instead of a runtime 404.
macro_rules! define_id {
    ($name:ident) => {
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, ts_rs::TS)]
        #[ts(type = "string")]
        pub struct $name(Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            pub fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            pub fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(Uuid::parse_str(s)?))
            }
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0.to_string())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let raw = String::deserialize(deserializer)?;
                Uuid::parse_str(&raw)
                    .map(Self)
                    .map_err(serde::de::Error::custom)
            }
        }
    };
}

define_id!(ThreadId);
define_id!(TurnId);
define_id!(ItemId);
define_id!(ChildId);
define_id!(RunId);
define_id!(AttachmentId);

/// Stable synthetic id for an event whose render identity is a string key.
pub fn keyed_item_id(namespace: &str) -> ItemId {
    let hi = fnv1a(namespace.as_bytes(), 0xcbf2_9ce4_8422_2325);
    let lo = fnv1a(namespace.as_bytes(), 0x8422_2325_cbf2_9ce4);
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&hi.to_be_bytes());
    bytes[8..].copy_from_slice(&lo.to_be_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    ItemId::from_uuid(Uuid::from_bytes(bytes))
}

fn fnv1a(bytes: &[u8], seed: u64) -> u64 {
    let mut hash = seed;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_serialize_as_plain_strings() {
        let id = ThreadId::new();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{}\"", id));
        let back: ThreadId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn ids_reject_garbage() {
        assert!(serde_json::from_str::<TurnId>("\"not-a-uuid\"").is_err());
    }

    #[test]
    fn keyed_item_ids_are_stable_across_producers() {
        let id = keyed_item_id("turn_failed:00000000-0000-0000-0000-000000000003");
        assert_eq!(id.to_string(), "47bfd081-5592-8d05-a602-cad4686b2174");
    }
}
