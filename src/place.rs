//! Turnstone's data-only shared-place vocabulary.
//!
//! This module deliberately contains no Gemot, Commons, transport, store, or
//! key types. Those live behind the shell-owned [`worker`] while this module
//! carries the durable public binding and app-owned state used by surfaces.

use serde::{Deserialize, Serialize};

pub mod invite;
pub(crate) mod lanes;
pub mod projection;
pub mod worker;

/// The only binding version this Turnstone build understands.
pub const PLACE_BINDING_VERSION: u16 = 1;

fn encode_hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex(kind: &'static str, value: &str) -> Result<[u8; 32], PlaceIdError> {
    if value.len() != 64 {
        return Err(PlaceIdError {
            kind,
            reason: "expected 64 lowercase hexadecimal characters",
        });
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(PlaceIdError {
            kind,
            reason: "identifier contains a non-lowercase-hexadecimal character",
        });
    }
    let mut bytes = [0; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(pair).map_err(|_| PlaceIdError {
            kind,
            reason: "identifier is not UTF-8 hexadecimal",
        })?;
        bytes[index] =
            u8::from_str_radix(text, 16).expect("validated hexadecimal pair always parses");
    }
    Ok(bytes)
}

/// A malformed serialized place identifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaceIdError {
    kind: &'static str,
    reason: &'static str,
}

impl std::fmt::Display for PlaceIdError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "invalid {}: {}", self.kind, self.reason)
    }
}

impl std::error::Error for PlaceIdError {}

macro_rules! place_id {
    ($name:ident, $kind:literal) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub [u8; 32]);

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                encode_hex(&value.0)
            }
        }

        impl TryFrom<String> for $name {
            type Error = PlaceIdError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                decode_hex($kind, &value).map(Self)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(&encode_hex(&self.0))
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::try_from(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

place_id!(PlaceId, "Moot id");
place_id!(SharedContainerId, "Commons root container id");
place_id!(ChatSpaceId, "Commons chat space id");

/// The public, durable binding between one Turnstone session and one governed
/// shared place.
///
/// These ids remain distinct because the live domain APIs address different
/// replicated spaces with them. Knot documents are addressed nodes inside the
/// shared graph and therefore need no fourth root here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceBindingV1 {
    pub version: u16,
    pub moot: PlaceId,
    pub root: SharedContainerId,
    pub chat: ChatSpaceId,
    pub default_channel: String,
}

impl PlaceBindingV1 {
    pub fn new(
        moot: PlaceId,
        root: SharedContainerId,
        chat: ChatSpaceId,
        default_channel: impl Into<String>,
    ) -> Result<Self, PlaceBindingError> {
        let binding = Self {
            version: PLACE_BINDING_VERSION,
            moot,
            root,
            chat,
            default_channel: default_channel.into(),
        };
        binding.validate()?;
        Ok(binding)
    }

    /// Validate product-level bounds after deserialization. Domain membership
    /// and authority are intentionally not guessed here.
    pub fn validate(&self) -> Result<(), PlaceBindingError> {
        if self.version != PLACE_BINDING_VERSION {
            return Err(PlaceBindingError::UnsupportedVersion(self.version));
        }
        if self.default_channel.trim().is_empty() {
            return Err(PlaceBindingError::EmptyDefaultChannel);
        }
        if self.default_channel.len() > 128 {
            return Err(PlaceBindingError::DefaultChannelTooLong(
                self.default_channel.len(),
            ));
        }
        if self.default_channel.chars().any(char::is_control) {
            return Err(PlaceBindingError::DefaultChannelHasControl);
        }
        Ok(())
    }
}

/// A malformed or unsupported public place binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaceBindingError {
    UnsupportedVersion(u16),
    EmptyDefaultChannel,
    DefaultChannelTooLong(usize),
    DefaultChannelHasControl,
}

impl std::fmt::Display for PlaceBindingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported place binding version {version}")
            }
            Self::EmptyDefaultChannel => write!(formatter, "default channel is empty"),
            Self::DefaultChannelTooLong(length) => {
                write!(
                    formatter,
                    "default channel is {length} bytes; maximum is 128"
                )
            }
            Self::DefaultChannelHasControl => {
                write!(formatter, "default channel contains a control character")
            }
        }
    }
}

impl std::error::Error for PlaceBindingError {}

/// Cached, app-owned summary of the retained place domains.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OfflinePlaceSnapshot {
    pub moot: MootCache,
    pub graph: GraphCache,
    pub chat: ChatCache,
    pub group: GroupCache,
    /// What the shared graph actually holds, as opposed to how much of it.
    /// Already authority-filtered: see [`projection::SharedGraph`].
    pub shared: projection::SharedGraph,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MootCache {
    pub membership_epoch: u64,
    pub members: usize,
    pub roster_members: usize,
    pub delegated_certificates: usize,
    pub tessera_operations: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GraphCache {
    pub nodes: usize,
    pub edges: usize,
    pub pending_causality: usize,
    pub pending_authority: usize,
    pub revoked_authority: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChatCache {
    pub channels: usize,
    pub messages: usize,
    pub deleted_messages: usize,
    pub pending_causality: usize,
    pub pending_authority: usize,
    pub revoked_authority: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupCache {
    pub members: usize,
    pub epochs: usize,
    pub has_current_epoch: bool,
}

/// Current product-visible state of the session's shared place.
///
/// Every worker-backed state carries the generation that opened it. A service
/// answer from a departed session or an earlier open therefore cannot mutate
/// the active app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaceState {
    Personal,
    /// An invitation is being admitted.
    ///
    /// Deliberately carries no binding. The envelope names one, but until every
    /// admission check has answered there is no admitted place, and app state
    /// claiming a binding here would be the same conflation the persistence
    /// ordering exists to prevent.
    Joining {
        generation: u64,
    },
    Opening {
        binding: PlaceBindingV1,
        generation: u64,
    },
    Offline {
        binding: PlaceBindingV1,
        generation: u64,
        snapshot: OfflinePlaceSnapshot,
    },
    Degraded {
        binding: PlaceBindingV1,
        generation: u64,
        error: String,
    },
    Failed {
        error: String,
    },
}

impl Default for PlaceState {
    fn default() -> Self {
        Self::Personal
    }
}

impl PlaceState {
    pub fn binding(&self) -> Option<&PlaceBindingV1> {
        match self {
            Self::Opening { binding, .. }
            | Self::Offline { binding, .. }
            | Self::Degraded { binding, .. } => Some(binding),
            Self::Personal | Self::Joining { .. } | Self::Failed { .. } => None,
        }
    }

    pub fn generation(&self) -> Option<u64> {
        match self {
            Self::Joining { generation }
            | Self::Opening { generation, .. }
            | Self::Offline { generation, .. }
            | Self::Degraded { generation, .. } => Some(*generation),
            Self::Personal | Self::Failed { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> PlaceBindingV1 {
        PlaceBindingV1::new(
            PlaceId([1; 32]),
            SharedContainerId([2; 32]),
            ChatSpaceId([3; 32]),
            "hall",
        )
        .unwrap()
    }

    #[test]
    fn binding_json_uses_distinct_readable_hex_ids() {
        let json = serde_json::to_string_pretty(&binding()).unwrap();
        assert!(json.contains(&"01".repeat(32)));
        assert!(json.contains(&"02".repeat(32)));
        assert!(json.contains(&"03".repeat(32)));
        assert!(!json.contains("[\n    1,"));
        let restored: PlaceBindingV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, binding());
    }

    #[test]
    fn binding_validation_rejects_version_and_channel_ambiguity() {
        let mut candidate = binding();
        candidate.version = 2;
        assert_eq!(
            candidate.validate(),
            Err(PlaceBindingError::UnsupportedVersion(2))
        );

        candidate.version = PLACE_BINDING_VERSION;
        candidate.default_channel = " \t".into();
        assert_eq!(
            candidate.validate(),
            Err(PlaceBindingError::EmptyDefaultChannel)
        );

        candidate.default_channel = "hall\nannouncements".into();
        assert_eq!(
            candidate.validate(),
            Err(PlaceBindingError::DefaultChannelHasControl)
        );
    }

    #[test]
    fn serialized_id_requires_exact_hex() {
        let error = serde_json::from_str::<PlaceId>("\"abcd\"").unwrap_err();
        assert!(error.to_string().contains("expected 64"));
        let error =
            serde_json::from_str::<PlaceId>(&format!("\"{}g\"", "0".repeat(63))).unwrap_err();
        assert!(error.to_string().contains("non-lowercase-hexadecimal"));
        let error =
            serde_json::from_str::<PlaceId>(&format!("\"{}A\"", "0".repeat(63))).unwrap_err();
        assert!(error.to_string().contains("non-lowercase-hexadecimal"));
    }
}
