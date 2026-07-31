//! The place invitation envelope.
//!
//! This is a host envelope, never a certificate. Holding one, forwarding one,
//! or parsing one successfully grants nothing: it names a place and carries the
//! artifacts an admission check needs, and every one of those artifacts is
//! verified by its own domain before anything is persisted.
//!
//! Everything here is data and bounds. The admission decision lives in the
//! worker, where Gemot and Stickleback can actually answer.
//!
//! **Delivery is not evidence.** An invitation may reasonably arrive over a
//! Murm thread with someone the user already knows and trusts, over a pasted
//! link, or over a radio carrier. None of that changes admission: the five
//! checks are identical, and a trusted sender shortcuts none of them. This is
//! structural rather than a rule to remember, because `admit_invitation` takes
//! no channel, peer, or session argument. There is nowhere for delivery trust
//! to enter it, and there should remain nowhere.

use serde::{Deserialize, Serialize};

use crate::place::{PlaceBindingV1, PlaceBindingError};

/// The only invitation version this Turnstone build understands.
pub const PLACE_INVITE_VERSION: u16 = 1;

/// The one rendezvous carrier this build dials. Unknown tags survive parsing
/// so an invitation authored by a newer peer stays usable for its recognized
/// carriers, but they are never dialed.
pub const P2PANDA_ENDPOINT_TICKET: &str = "p2panda.endpoint-ticket.v1";

/// Bounds applied before allocation or fetch. An envelope arrives from a peer,
/// so these are refusal thresholds, not tuning.
const MAX_INLINE_ARTIFACT_BYTES: usize = 1024 * 1024;
const MAX_MEDIA_TYPE_LEN: usize = 128;
const MAX_ADDRESS_LEN: usize = 2048;
const MAX_RENDEZVOUS: usize = 16;
const MAX_MEMBERSHIP_HEADS: usize = 256;
const MAX_CARRIER_LEN: usize = 128;
const MAX_HINT_LEN: usize = 4096;

/// One artifact an invitation carries or points at.
///
/// The digest is the caller's declared identity for the bytes. It is checked
/// against the bytes here; whether those bytes *mean* anything is the owning
/// domain's question, not this module's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactRefV1 {
    Inline {
        media_type: String,
        digest: [u8; 32],
        bytes: Vec<u8>,
    },
    Addressed {
        media_type: String,
        digest: [u8; 32],
        address: String,
    },
}

impl ArtifactRefV1 {
    pub fn media_type(&self) -> &str {
        match self {
            Self::Inline { media_type, .. } | Self::Addressed { media_type, .. } => media_type,
        }
    }

    pub fn digest(&self) -> &[u8; 32] {
        match self {
            Self::Inline { digest, .. } | Self::Addressed { digest, .. } => digest,
        }
    }

    /// Bounded, digest-checked bytes, or the reason they cannot be trusted.
    ///
    /// An `Addressed` artifact has nothing to check yet. It returns
    /// [`InviteError::UnfetchedArtifact`] rather than an empty success, so a
    /// caller cannot mistake "not fetched" for "verified empty".
    pub fn verified_bytes(&self, field: &'static str) -> Result<&[u8], InviteError> {
        match self {
            Self::Inline {
                digest,
                bytes,
                media_type,
            } => {
                if media_type.len() > MAX_MEDIA_TYPE_LEN {
                    return Err(InviteError::FieldTooLong {
                        field,
                        length: media_type.len(),
                        maximum: MAX_MEDIA_TYPE_LEN,
                    });
                }
                if bytes.len() > MAX_INLINE_ARTIFACT_BYTES {
                    return Err(InviteError::ArtifactTooLarge {
                        field,
                        length: bytes.len(),
                        maximum: MAX_INLINE_ARTIFACT_BYTES,
                    });
                }
                let actual = proofs::Digest::blake3(bytes);
                if actual.bytes.as_slice() != digest.as_slice() {
                    return Err(InviteError::ArtifactDigestMismatch { field });
                }
                Ok(bytes)
            }
            Self::Addressed {
                media_type,
                address,
                ..
            } => {
                if media_type.len() > MAX_MEDIA_TYPE_LEN {
                    return Err(InviteError::FieldTooLong {
                        field,
                        length: media_type.len(),
                        maximum: MAX_MEDIA_TYPE_LEN,
                    });
                }
                if address.len() > MAX_ADDRESS_LEN {
                    return Err(InviteError::FieldTooLong {
                        field,
                        length: address.len(),
                        maximum: MAX_ADDRESS_LEN,
                    });
                }
                Err(InviteError::UnfetchedArtifact { field })
            }
        }
    }
}

/// One carrier hint. Opaque to this module: the carrier tag decides whether the
/// hint is a ticket, an address, or something this build has never heard of.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RendezvousV1 {
    pub carrier: String,
    pub hint: String,
}

/// A named place plus the evidence an admission check needs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceInviteV1 {
    pub version: u16,
    pub binding: PlaceBindingV1,
    /// Personae root that founded the Moot.
    ///
    /// Distinct from `inviter`, and not a trust decision: Gemot's genesis
    /// admission requires the retained genesis to be authored by, and to name,
    /// exactly this root, so a false claim is refused by signature rather than
    /// believed. It is carried because importing a drop needs the founder
    /// before the fold exists, and founder discovery needs the fold.
    ///
    /// Conflating this with `inviter` was a real bug: an invitation from an
    /// ordinary member, which is the normal case, would have been refused with
    /// a founder error.
    pub founder: [u8; 32],
    /// Personae root that authored the welcome.
    ///
    /// Added 2026-07-31; the plan's original struct omitted it, and Stickleback
    /// requires an authenticated author root to process a control frame. This
    /// field is not trusted on its own: `inviter_prekey` attests the same root
    /// cryptographically, and admission requires the two to agree *and* the
    /// root to appear in the Gemot membership fold.
    pub inviter: [u8; 32],
    /// The inviter's published `GroupPrekeyBundle`.
    ///
    /// Needed because a welcome cannot be processed without the sender's
    /// authenticated pre-key, and a freshly prepared group identity knows only
    /// its own. The bundle carries a Personae-signed attestation, so it is also
    /// what turns `inviter` from a claim into a verified fact.
    pub inviter_prekey: ArtifactRefV1,
    /// Signed Gemot bootstrap evidence, first shaped as an aggregate native drop.
    pub governance: ArtifactRefV1,
    /// The broadcast half of the Stickleback welcome: one `GroupControlFrame`.
    ///
    /// Never a serialized `DataKeyring`. A raw keyring would admit whoever held
    /// the envelope, which is the whole failure this split exists to prevent.
    pub key_welcome: ArtifactRefV1,
    /// The recipient-bound half: one `GroupDirectFrame` addressed to the
    /// invitee's authenticated crypto identity.
    ///
    /// Split from `key_welcome` on 2026-07-31 rather than encoded together as a
    /// `GroupSessionDispatch`. Stickleback publishes bounded, version-checking
    /// decoders for each frame and none for the dispatch, so carrying them
    /// separately means a peer-supplied welcome is parsed by its own domain
    /// instead of by a raw CBOR decode into a struct with private fields.
    pub key_direct: ArtifactRefV1,
    /// The group epoch this welcome must install.
    ///
    /// A `GroupSecretId` is the SHA-256 of the secret, not the secret, so
    /// naming one discloses nothing and this module keeps its no-key-types
    /// discipline. It is carried so admission can refuse a welcome that
    /// installs some *other* epoch than the one the inviter is describing.
    pub expected_epoch: [u8; 32],
    /// The Gemot membership heads the epoch was minted against, sorted.
    ///
    /// This is the fifth check's substance: an epoch minted before a removal
    /// still decrypts, so without pinning the membership state it was minted
    /// at, a welcome could hand a joiner a key that a since-departed member
    /// also holds. Admission requires these to equal the heads Gemot itself
    /// converged to from the imported evidence.
    pub membership_heads: Vec<[u8; 32]>,
    /// When this invitation stops being offered, in milliseconds since the
    /// Unix epoch.
    ///
    /// Deliberately separate from `membership_heads`, which already makes an
    /// invitation stale the moment the roster changes. They answer different
    /// questions: the heads pin is a security bound the domain enforces and
    /// cannot be relaxed, while this is a time bound the inviter chooses, so a
    /// forwarded envelope stops working even in a Moot whose membership never
    /// moves. Checked against the host's `AuthorityClock`, like every other
    /// time-dependent place decision.
    pub not_after_ms: u64,
    pub rendezvous: Vec<RendezvousV1>,
}

impl PlaceInviteV1 {
    /// Structural validation only. Everything a peer could lie about that this
    /// module can check locally, and nothing it cannot.
    ///
    /// Passing this is not admission. It means the envelope is well formed
    /// enough to spend a domain check on.
    pub fn validate(&self) -> Result<(), InviteError> {
        if self.version != PLACE_INVITE_VERSION {
            return Err(InviteError::UnsupportedVersion(self.version));
        }
        self.binding.validate().map_err(InviteError::Binding)?;
        if self.membership_heads.is_empty() {
            return Err(InviteError::NoMembershipHeads);
        }
        if self.membership_heads.len() > MAX_MEMBERSHIP_HEADS {
            return Err(InviteError::FieldTooLong {
                field: "membership heads",
                length: self.membership_heads.len(),
                maximum: MAX_MEMBERSHIP_HEADS,
            });
        }
        if self.rendezvous.len() > MAX_RENDEZVOUS {
            return Err(InviteError::TooManyRendezvous {
                count: self.rendezvous.len(),
                maximum: MAX_RENDEZVOUS,
            });
        }
        for entry in &self.rendezvous {
            if entry.carrier.trim().is_empty() {
                return Err(InviteError::EmptyCarrier);
            }
            if entry.carrier.len() > MAX_CARRIER_LEN {
                return Err(InviteError::FieldTooLong {
                    field: "rendezvous carrier",
                    length: entry.carrier.len(),
                    maximum: MAX_CARRIER_LEN,
                });
            }
            if entry.hint.len() > MAX_HINT_LEN {
                return Err(InviteError::FieldTooLong {
                    field: "rendezvous hint",
                    length: entry.hint.len(),
                    maximum: MAX_HINT_LEN,
                });
            }
        }
        // Bounds and digests, so a later admission step never faces unbounded
        // or misdeclared bytes. An addressed artifact is not a validation
        // failure here; it becomes one when admission needs the bytes.
        for (artifact, field) in [
            (&self.governance, "governance artifact"),
            (&self.key_welcome, "key welcome artifact"),
            (&self.key_direct, "recipient welcome artifact"),
            (&self.inviter_prekey, "inviter pre-key artifact"),
        ] {
            match artifact.verified_bytes(field) {
                Ok(_) | Err(InviteError::UnfetchedArtifact { .. }) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    /// Rendezvous entries this build knows how to dial, in envelope order.
    ///
    /// Unknown carriers are dropped rather than refused: a newer peer may offer
    /// carriers alongside ones we recognize, and refusing the whole envelope
    /// would make every carrier addition a breaking change.
    pub fn dialable(&self) -> impl Iterator<Item = &RendezvousV1> {
        self.rendezvous
            .iter()
            .filter(|entry| entry.carrier == P2PANDA_ENDPOINT_TICKET)
    }
}

/// A refused invitation. Every variant names what was wrong without echoing
/// peer-supplied bytes back into a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InviteError {
    UnsupportedVersion(u16),
    Binding(PlaceBindingError),
    TooManyRendezvous {
        count: usize,
        maximum: usize,
    },
    EmptyCarrier,
    NoMembershipHeads,
    FieldTooLong {
        field: &'static str,
        length: usize,
        maximum: usize,
    },
    ArtifactTooLarge {
        field: &'static str,
        length: usize,
        maximum: usize,
    },
    ArtifactDigestMismatch {
        field: &'static str,
    },
    UnfetchedArtifact {
        field: &'static str,
    },
}

impl std::fmt::Display for InviteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported place invite version {version}")
            }
            Self::Binding(error) => write!(formatter, "invitation binding: {error}"),
            Self::TooManyRendezvous { count, maximum } => write!(
                formatter,
                "invitation carries {count} rendezvous entries; maximum is {maximum}"
            ),
            Self::EmptyCarrier => write!(formatter, "rendezvous carrier tag is empty"),
            Self::NoMembershipHeads => write!(
                formatter,
                "invitation pins no membership heads for its group epoch"
            ),
            Self::FieldTooLong {
                field,
                length,
                maximum,
            } => write!(
                formatter,
                "{field} is {length} bytes; maximum is {maximum}"
            ),
            Self::ArtifactTooLarge {
                field,
                length,
                maximum,
            } => write!(
                formatter,
                "{field} is {length} bytes; maximum is {maximum}"
            ),
            Self::ArtifactDigestMismatch { field } => {
                write!(formatter, "{field} does not match its declared digest")
            }
            Self::UnfetchedArtifact { field } => {
                write!(formatter, "{field} is addressed and has not been fetched")
            }
        }
    }
}

impl std::error::Error for InviteError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::place::{ChatSpaceId, PlaceId, SharedContainerId};

    fn inline(bytes: &[u8]) -> ArtifactRefV1 {
        let digest = proofs::Digest::blake3(bytes);
        ArtifactRefV1::Inline {
            media_type: "application/vnd.mere.gemot-drop".into(),
            digest: digest.bytes.as_slice().try_into().unwrap(),
            bytes: bytes.to_vec(),
        }
    }

    fn invite() -> PlaceInviteV1 {
        PlaceInviteV1 {
            version: PLACE_INVITE_VERSION,
            binding: PlaceBindingV1::new(
                PlaceId([1; 32]),
                SharedContainerId([2; 32]),
                ChatSpaceId([3; 32]),
                "hall",
            )
            .unwrap(),
            founder: [8; 32],
            inviter: [9; 32],
            inviter_prekey: inline(b"inviter pre-key bundle"),
            governance: inline(b"gemot native drop"),
            key_welcome: inline(b"group control frame"),
            key_direct: inline(b"recipient-bound direct frame"),
            expected_epoch: [4; 32],
            membership_heads: vec![[5; 32]],
            not_after_ms: 1_000,
            rendezvous: vec![RendezvousV1 {
                carrier: P2PANDA_ENDPOINT_TICKET.into(),
                hint: "ticket".into(),
            }],
        }
    }

    #[test]
    fn a_well_formed_invitation_round_trips_and_validates() {
        let invite = invite();
        invite.validate().unwrap();
        let json = serde_json::to_string(&invite).unwrap();
        let restored: PlaceInviteV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, invite);
        assert_eq!(restored.dialable().count(), 1);
    }

    #[test]
    fn a_tampered_artifact_is_refused_by_its_own_digest() {
        let mut invite = invite();
        let ArtifactRefV1::Inline { bytes, .. } = &mut invite.governance else {
            unreachable!("fixture is inline")
        };
        bytes.push(0);
        assert_eq!(
            invite.validate(),
            Err(InviteError::ArtifactDigestMismatch {
                field: "governance artifact"
            })
        );
    }

    #[test]
    fn an_oversized_inline_artifact_is_refused_before_it_is_trusted() {
        let mut invite = invite();
        invite.key_welcome = inline(&vec![0u8; MAX_INLINE_ARTIFACT_BYTES + 1]);
        assert_eq!(
            invite.validate(),
            Err(InviteError::ArtifactTooLarge {
                field: "key welcome artifact",
                length: MAX_INLINE_ARTIFACT_BYTES + 1,
                maximum: MAX_INLINE_ARTIFACT_BYTES,
            })
        );
    }

    #[test]
    fn an_unknown_carrier_survives_parsing_but_is_never_dialed() {
        let mut invite = invite();
        invite.rendezvous.insert(
            0,
            RendezvousV1 {
                carrier: "retinue.some-future-carrier.v9".into(),
                hint: "unknown".into(),
            },
        );
        invite.validate().unwrap();
        let dialable: Vec<_> = invite.dialable().collect();
        assert_eq!(dialable.len(), 1);
        assert_eq!(dialable[0].carrier, P2PANDA_ENDPOINT_TICKET);
    }

    #[test]
    fn an_addressed_artifact_never_reads_as_verified_empty() {
        let mut invite = invite();
        invite.governance = ArtifactRefV1::Addressed {
            media_type: "application/vnd.mere.gemot-drop".into(),
            digest: [7; 32],
            address: "https://example.invalid/drop".into(),
        };
        // Structural validation tolerates it: nothing is wrong with the
        // envelope yet.
        invite.validate().unwrap();
        // Admission cannot: asking for the bytes is an explicit refusal, not
        // an empty slice.
        assert_eq!(
            invite.governance.verified_bytes("governance artifact"),
            Err(InviteError::UnfetchedArtifact {
                field: "governance artifact"
            })
        );
    }

    #[test]
    fn version_and_binding_faults_are_named_distinctly() {
        let mut candidate = invite();
        candidate.version = 2;
        assert_eq!(
            candidate.validate(),
            Err(InviteError::UnsupportedVersion(2))
        );

        let mut candidate = invite();
        candidate.binding.default_channel = String::new();
        assert_eq!(
            candidate.validate(),
            Err(InviteError::Binding(PlaceBindingError::EmptyDefaultChannel))
        );
    }

    #[test]
    fn rendezvous_bounds_refuse_a_flooded_envelope() {
        let mut candidate = invite();
        candidate.rendezvous = (0..MAX_RENDEZVOUS + 1)
            .map(|index| RendezvousV1 {
                carrier: format!("carrier-{index}"),
                hint: String::new(),
            })
            .collect();
        assert_eq!(
            candidate.validate(),
            Err(InviteError::TooManyRendezvous {
                count: MAX_RENDEZVOUS + 1,
                maximum: MAX_RENDEZVOUS,
            })
        );

        let mut candidate = invite();
        candidate.rendezvous = vec![RendezvousV1 {
            carrier: "  ".into(),
            hint: String::new(),
        }];
        assert_eq!(candidate.validate(), Err(InviteError::EmptyCarrier));
    }
}
