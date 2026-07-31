//! Shell-owned retained-place worker.
//!
//! This is the product composition boundary. It opens Gemot, Commons graph,
//! Commons chat, and Stickleback group state for one Turnstone session, then
//! emits only app-owned summaries tagged with the session and open generation.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use armillary::{ActorHandle, Emitter, Wake, spawn_named};
use commons::chat::ChatReplica;
use commons::{GemotAuthorityView, Replica};
use gemot::moot::{
    AvailabilityPolicy, ErasurePolicy, KeepBound, MootAuthority, MootFile, MootId,
    MootRetentionSettings, PolicyRevision,
};
use identity::{IdentityProvider, SealedRecordStorage};
use muniment::RedbBackend;
use proofs::Digest;
use stickleback::{DataKeyring, GroupSession, GroupSessionId};

use crate::action::Update;
use crate::identity::RootIdentity;
use crate::panes::SessionId;
use crate::place::{
    ChatCache, GraphCache, GroupCache, MootCache, OfflinePlaceSnapshot, PlaceBindingV1,
};

const GROUP_SESSION_RECORD: &str = "group.session";

/// Host-set evaluation time for converged authority.
///
/// Delegation grants and revocations carry absolute windows, so this value is
/// what decides whether an unauthorized operation reads as pending or revoked.
/// It is a host input on purpose: session, relay, and transport identity never
/// enter the decision. Tests pin it so a verdict is reproducible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorityClock {
    SystemTime,
    Fixed(u64),
}

impl AuthorityClock {
    fn now_ms(self) -> u64 {
        match self {
            // A clock behind the epoch yields 0, under which no grant has
            // opened yet, so every retained fact reads as pending rather than
            // effective. Unreadable time withholds content, it does not admit it.
            Self::SystemTime => std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis() as u64),
            Self::Fixed(at_ms) => at_ms,
        }
    }
}

/// Runtime settings for the offline worker. The default is deliberately
/// conservative: opening cached state never proposes expiry or erasure.
#[derive(Clone, Debug)]
pub struct PlaceWorkerSettings {
    pub retention: MootRetentionSettings,
    pub authority_clock: AuthorityClock,
}

impl Default for PlaceWorkerSettings {
    fn default() -> Self {
        Self {
            authority_clock: AuthorityClock::SystemTime,
            retention: MootRetentionSettings {
                revision: PolicyRevision(Digest::blake3(b"turnstone.place.offline-retention.v1")),
                availability: AvailabilityPolicy {
                    promised_floor: KeepBound::Forever,
                },
                erasure: ErasurePolicy {
                    history_ceiling: KeepBound::Forever,
                },
            },
        }
    }
}

/// Commands accepted by the one shell-owned place worker.
pub enum PlaceWorkerCommand {
    Open {
        session: SessionId,
        generation: u64,
        directory: PathBuf,
        binding: PlaceBindingV1,
    },
    Release(std::sync::mpsc::SyncSender<()>),
}

struct OpenPlace {
    _moot: MootFile,
    _graph: Replica<RedbBackend>,
    _chat: ChatReplica<RedbBackend>,
    _group: GroupSession,
}

pub fn place_store_dir(session_dir: &Path) -> PathBuf {
    session_dir.join("place")
}

pub fn place_secrets_dir(session_dir: &Path) -> PathBuf {
    session_dir.join("place-secrets")
}

fn place_secret_salt(moot: [u8; 32]) -> Vec<u8> {
    let mut salt = Vec::with_capacity(59);
    salt.extend_from_slice(b"turnstone.place.secrets.v1/");
    salt.extend_from_slice(&moot);
    salt
}

fn sealed_group_store(
    session_dir: &Path,
    identity: &dyn IdentityProvider,
    moot: [u8; 32],
) -> Result<SealedRecordStorage, String> {
    let key = identity
        .derive_keypair(&place_secret_salt(moot))
        .map_err(|error| format!("derive place secret key: {error}"))?
        .to_seed();
    Ok(SealedRecordStorage::open_with_key(
        place_secrets_dir(session_dir),
        key,
    ))
}

/// Persist the group session inside the same Personae-derived sealed boundary
/// the worker reopens. Invitation import will call this after validation.
pub fn save_group_session(
    session_dir: &Path,
    identity: &dyn IdentityProvider,
    session: &GroupSession,
) -> Result<(), String> {
    let storage = sealed_group_store(session_dir, identity, session.group().0)?;
    let bytes = session
        .to_bytes()
        .map_err(|error| format!("encode group session: {error}"))?;
    storage
        .save_record(GROUP_SESSION_RECORD, &bytes)
        .map_err(|error| format!("seal group session: {error}"))
}

fn open_cached_place(
    directory: &Path,
    binding: &PlaceBindingV1,
    identity: &dyn IdentityProvider,
    settings: &PlaceWorkerSettings,
) -> Result<(OpenPlace, OfflinePlaceSnapshot), String> {
    binding
        .validate()
        .map_err(|error| format!("place binding: {error}"))?;
    let storage = sealed_group_store(directory, identity, binding.moot.0)?;
    let bytes: Vec<u8> = storage
        .load_record(GROUP_SESSION_RECORD)
        .map_err(|error| format!("load sealed group session: {error}"))?
        .ok_or_else(|| "sealed group session is absent".to_string())?;
    let group = GroupSession::from_bytes(&bytes)
        .map_err(|error| format!("decode sealed group session: {error}"))?;
    if group.group() != GroupSessionId(binding.moot.0) {
        return Err("sealed group session addresses another Moot".to_string());
    }
    let root = identity.master_public_key().to_bytes();
    if group.personae_root() != root {
        return Err("sealed group session belongs to another Personae root".to_string());
    }
    let keyring = DataKeyring::from_bytes(
        &group
            .data_keyring_state()
            .map_err(|error| format!("read group data epochs: {error}"))?,
    )
    .map_err(|error| format!("decode group data epochs: {error}"))?;

    let stores = place_store_dir(directory);
    let moot = pollster::block_on(MootFile::open_existing(
        stores.join("gemot"),
        MootId(binding.moot.0),
        settings.retention.clone(),
    ))
    .map_err(|error| format!("open Gemot cache: {error}"))?;
    let moot_snapshot = pollster::block_on(moot.snapshot())
        .map_err(|error| format!("materialize Gemot: {error}"))?;

    // The single authority view both Commons domains project through. It is
    // built from the Moot's own converged constitution and delegation fold, so
    // an operation whose author was never granted, or whose grant was
    // withdrawn, cannot reach a projection this worker emits.
    let delegations = pollster::block_on(moot.delegations())
        .map_err(|error| format!("materialize Gemot delegations: {error}"))?;
    let authority = GemotAuthorityView {
        authority: MootAuthority {
            delegations: &delegations,
            rules: &moot_snapshot.governance.rules,
            moot_id: binding.moot.0,
            now_ms: settings.authority_clock.now_ms(),
        },
    };

    let graph_backend = RedbBackend::open(stores.join("commons-graph.redb"))
        .map_err(|error| format!("open Commons graph cache: {error}"))?;
    let graph = Replica::for_identity(graph_backend, binding.root.0, identity)
        .map_err(|error| format!("bind Commons graph writer: {error}"))?;
    let graph_projection = pollster::block_on(graph.projection_with_authority(&authority))
        .map_err(|error| format!("materialize Commons graph: {error}"))?;

    let chat_backend = RedbBackend::open(stores.join("commons-chat.redb"))
        .map_err(|error| format!("open Commons chat cache: {error}"))?;
    let chat = ChatReplica::for_identity(chat_backend, binding.chat.0, identity, keyring)
        .map_err(|error| format!("bind Commons chat writer: {error}"))?;
    let chat_projection = pollster::block_on(chat.projection_with_authority(&authority))
        .map_err(|error| format!("materialize Commons chat: {error}"))?;

    let group_members = group
        .members()
        .map_err(|error| format!("materialize group membership: {error}"))?
        .len();
    let snapshot = OfflinePlaceSnapshot {
        moot: MootCache {
            membership_epoch: moot_snapshot.membership.epoch,
            members: moot_snapshot.membership.members.len(),
            roster_members: moot_snapshot.roster.members.len(),
            delegated_certificates: moot_snapshot.delegated_certificates,
            tessera_operations: moot_snapshot.tessera_operations,
        },
        graph: GraphCache {
            nodes: graph_projection.graph.graph().node_count(),
            edges: graph_projection.graph.graph().edge_count(),
            pending_causality: graph_projection.pending.len(),
            pending_authority: graph_projection.pending_authority.len(),
            revoked_authority: graph_projection.revoked.len(),
        },
        chat: ChatCache {
            channels: chat_projection.channels.len(),
            messages: chat_projection.messages.len(),
            deleted_messages: chat_projection.deleted_messages.len(),
            pending_causality: chat_projection.pending.len(),
            pending_authority: chat_projection.pending_authority.len(),
            revoked_authority: chat_projection.revoked.len(),
        },
        group: GroupCache {
            members: group_members,
            epochs: group.epoch_count(),
            has_current_epoch: group.current_epoch().is_some(),
        },
    };
    Ok((
        OpenPlace {
            _moot: moot,
            _graph: graph,
            _chat: chat,
            _group: group,
        },
        snapshot,
    ))
}

/// Spawn the retained-place worker. Each `Open` first releases the prior
/// session's database handles, so switch and trash can establish ordering with
/// the explicit `Release` acknowledgement.
pub fn spawn_place_worker(
    wake: Wake,
    identity: Arc<RootIdentity>,
    settings: PlaceWorkerSettings,
) -> (ActorHandle<PlaceWorkerCommand>, Receiver<Update>) {
    spawn_named(
        "turnstone-place",
        wake,
        move |commands, out: Emitter<Update>| {
            let mut live: Option<OpenPlace> = None;
            while let Ok(command) = commands.recv() {
                match command {
                    PlaceWorkerCommand::Open {
                        session,
                        generation,
                        directory,
                        binding,
                    } => {
                        live = None;
                        match open_cached_place(&directory, &binding, identity.as_ref(), &settings)
                        {
                            Ok((opened, snapshot)) => {
                                live = Some(opened);
                                out.emit(Update::PlaceOpened {
                                    session,
                                    generation,
                                    result: Ok(snapshot),
                                });
                            }
                            Err(error) => out.emit(Update::PlaceOpened {
                                session,
                                generation,
                                result: Err(error),
                            }),
                        }
                    }
                    PlaceWorkerCommand::Release(ack) => {
                        live = None;
                        let _ = ack.send(());
                    }
                }
            }
            drop(live);
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chartulary::{Author, Container};
    use commons::chat::{Channel, ChatEvent, Message};
    use gemot::moot::constitution::{CapabilityGrant, ConstitutionRules};
    use gemot::moot::{MOOT_ACT_ACTION, MOOT_DELEGATION_DOMAIN};
    use identity::InMemoryProvider;
    use servitor::{Cap, cap_path};

    use identity::delegation::{
        CapabilityScope, DelegationCertificate, DelegationParent, DelegationRevocation,
        SignedDelegationCertificate, SignedDelegationRevocation, delegation_signing_salt,
    };

    use crate::place::{ChatSpaceId, PlaceId, SharedContainerId};

    /// Pinned so a delegation window, and therefore an authority verdict, is
    /// reproducible. `commons/container/...` and `commons/chat/...` both sit
    /// under the `commons` prefix these fixtures grant.
    const AUTHORITY_AT_MS: u64 = 50;
    const ROOT_GRANT: [u8; 32] = [0x67; 32];

    fn settings() -> PlaceWorkerSettings {
        PlaceWorkerSettings {
            authority_clock: AuthorityClock::Fixed(AUTHORITY_AT_MS),
            ..PlaceWorkerSettings::default()
        }
    }

    fn founder_for(binding: &PlaceBindingV1) -> InMemoryProvider {
        InMemoryProvider::from_seed([binding.moot.0[0].wrapping_add(40); 32])
    }

    /// `cap_path` encodes a scope as `scope/<path>`, and personae matches on a
    /// slash boundary, so this one prefix covers both `commons/container/...`
    /// and `commons/chat/...`. Deriving it beats writing the literal: the
    /// encoding is servitor's to change.
    fn place_capability_prefix() -> String {
        cap_path(&Cap::scope("commons").unwrap())
    }

    fn place_rules(founder_id: [u8; 32]) -> ConstitutionRules {
        let mut rules = ConstitutionRules::founder_only(founder_id);
        rules.grant(CapabilityGrant {
            id: ROOT_GRANT,
            subject: founder_id,
            path_prefix: place_capability_prefix(),
            not_before_ms: 10,
            expires_at_ms: Some(1_000),
            delegation_depth: 2,
        });
        rules
    }

    fn place_scope(moot: [u8; 32]) -> CapabilityScope {
        CapabilityScope {
            domain: MOOT_DELEGATION_DOMAIN.into(),
            resource: moot.to_vec(),
            path_prefix: place_capability_prefix(),
            actions: [MOOT_ACT_ACTION.to_string()].into_iter().collect(),
        }
    }

    /// Gemot authors delegation facts under the scope-derived key that signed
    /// the certificate, not the master key: the master secret stays behind the
    /// provider.
    fn founder_signing_key(founder: &InMemoryProvider, moot: [u8; 32]) -> identity::Ed25519Keypair {
        founder
            .derive_keypair(&delegation_signing_salt(&place_scope(moot)))
            .unwrap()
    }

    /// The founder's signed delegation admitting one profile root to both
    /// Commons domains. Deterministic, so a later test can recompute its id to
    /// revoke it.
    fn place_delegation(
        founder: &InMemoryProvider,
        moot: [u8; 32],
        subject: [u8; 32],
    ) -> SignedDelegationCertificate {
        SignedDelegationCertificate::issue(
            founder,
            DelegationCertificate::new(
                DelegationParent::Root(ROOT_GRANT),
                founder.master_public_key().to_bytes(),
                subject,
                place_scope(moot),
                15,
                20,
                Some(900),
                0,
                [1; 32],
            ),
        )
        .unwrap()
    }

    /// Withdraw the seeded delegation on the retained Gemot lane.
    fn revoke_place_delegation(directory: &Path, binding: &PlaceBindingV1, subject: [u8; 32]) {
        let founder = founder_for(binding);
        let founder_id = founder.master_public_key().to_bytes();
        let rules = place_rules(founder_id);
        let certificate = place_delegation(&founder, binding.moot.0, subject);
        let moot = pollster::block_on(MootFile::open(
            place_store_dir(directory).join("gemot"),
            MootId(binding.moot.0),
            founder_id,
            settings().retention,
        ))
        .unwrap();
        let revocation = SignedDelegationRevocation::issue(
            &founder,
            DelegationRevocation::new(
                certificate.certificate.id(),
                founder_id,
                certificate.certificate.scope.clone(),
                60,
                [2; 32],
            ),
        )
        .unwrap();
        pollster::block_on(moot.delegation_store().author_revoke(
            &founder_signing_key(&founder, binding.moot.0),
            &rules,
            revocation,
        ))
        .unwrap();
        drop(moot);
    }

    fn binding(seed: u8) -> PlaceBindingV1 {
        PlaceBindingV1::new(
            PlaceId([seed; 32]),
            SharedContainerId([seed.wrapping_add(1); 32]),
            ChatSpaceId([seed.wrapping_add(2); 32]),
            "hall",
        )
        .unwrap()
    }

    fn seed_profile(
        directory: &Path,
        identity: &RootIdentity,
        binding: &PlaceBindingV1,
        facts: usize,
    ) {
        let founder = founder_for(binding);
        let founder_id = founder.master_public_key().to_bytes();
        let settings = settings();
        let stores = place_store_dir(directory);
        std::fs::create_dir_all(&stores).unwrap();
        let rules = place_rules(founder_id);
        let moot = pollster::block_on(MootFile::open(
            stores.join("gemot"),
            MootId(binding.moot.0),
            founder_id,
            settings.retention.clone(),
        ))
        .unwrap();
        pollster::block_on(moot.found(
            founder.master_keypair().to_seed(),
            None,
            None,
            rules.clone(),
            1,
        ))
        .unwrap();
        // Without this the profile holds no capability and every fact it
        // authors below projects as pending, which is the correct verdict for
        // an unadmitted writer but not the fixture these tests need.
        pollster::block_on(moot.delegation_store().author_issue(
            &founder_signing_key(&founder, binding.moot.0),
            &rules,
            place_delegation(&founder, binding.moot.0, identity.master_public_key().to_bytes()),
        ))
        .unwrap();
        drop(moot);

        let (mut group, _) = GroupSession::new(GroupSessionId(binding.moot.0), identity).unwrap();
        group.create(&[]).unwrap();
        save_group_session(directory, identity, &group).unwrap();
        let keyring = DataKeyring::from_bytes(&group.data_keyring_state().unwrap()).unwrap();

        let graph_backend = RedbBackend::open(stores.join("commons-graph.redb")).unwrap();
        let mut graph = Replica::for_identity(graph_backend, binding.root.0, identity).unwrap();
        for index in 0..facts {
            pollster::block_on(graph.edit(|log| {
                log.insert_node(
                    &Author::new("turnstone"),
                    Container::new(format!("node-{index}")),
                );
            }))
            .unwrap();
        }
        drop(graph);

        let chat_backend = RedbBackend::open(stores.join("commons-chat.redb")).unwrap();
        let mut chat =
            ChatReplica::for_identity(chat_backend, binding.chat.0, identity, keyring).unwrap();
        pollster::block_on(chat.author(ChatEvent::Channel(Channel {
            id: "hall".into(),
            title: "Hall".into(),
        })))
        .unwrap();
        for index in 0..facts {
            pollster::block_on(chat.author(ChatEvent::Message(Message {
                channel: "hall".into(),
                body: format!("message {index}"),
                sent_at_ms: index as u64,
                reply_to: None,
            })))
            .unwrap();
        }
    }

    #[test]
    fn two_profiles_reopen_their_own_retained_place_state() {
        let root =
            std::env::temp_dir().join(format!("turnstone-place-worker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let first_dir = root.join("first");
        let second_dir = root.join("second");
        let first_identity = RootIdentity::Unsealed(InMemoryProvider::from_seed([0x81; 32]));
        let second_identity = RootIdentity::Unsealed(InMemoryProvider::from_seed([0x82; 32]));
        let first_binding = binding(0x21);
        let second_binding = binding(0x31);
        seed_profile(&first_dir, &first_identity, &first_binding, 1);
        seed_profile(&second_dir, &second_identity, &second_binding, 2);

        let (_, first) =
            open_cached_place(&first_dir, &first_binding, &first_identity, &settings()).unwrap();
        let (_, second) =
            open_cached_place(&second_dir, &second_binding, &second_identity, &settings()).unwrap();
        assert_eq!((first.graph.nodes, first.chat.messages), (1, 1));
        assert_eq!((second.graph.nodes, second.chat.messages), (2, 2));
        assert!(first.group.has_current_epoch);
        assert!(second.group.has_current_epoch);
        assert_eq!(first.moot.delegated_certificates, 1);
        assert_eq!(second.moot.delegated_certificates, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_revoked_member_reaches_no_projected_place_state() {
        let root =
            std::env::temp_dir().join(format!("turnstone-place-revoked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let directory = root.join("profile");
        let identity = RootIdentity::Unsealed(InMemoryProvider::from_seed([0xa1; 32]));
        let binding = binding(0x51);
        seed_profile(&directory, &identity, &binding, 2);

        let (_, admitted) =
            open_cached_place(&directory, &binding, &identity, &settings()).unwrap();
        assert_eq!((admitted.graph.nodes, admitted.chat.messages), (2, 2));
        assert_eq!(admitted.chat.channels, 1);
        assert_eq!(
            (
                admitted.graph.pending_authority,
                admitted.graph.revoked_authority,
                admitted.chat.pending_authority,
                admitted.chat.revoked_authority,
            ),
            (0, 0, 0, 0)
        );

        revoke_place_delegation(&directory, &binding, identity.master_public_key().to_bytes());

        let (_, withdrawn) =
            open_cached_place(&directory, &binding, &identity, &settings()).unwrap();
        assert_eq!(
            (withdrawn.graph.nodes, withdrawn.graph.edges),
            (0, 0),
            "a withdrawn member's graph facts must not reach PlaceState"
        );
        assert_eq!(
            (
                withdrawn.chat.messages,
                withdrawn.chat.channels,
                withdrawn.chat.deleted_messages
            ),
            (0, 0, 0),
            "a withdrawn member's chat facts must not reach PlaceState"
        );
        assert!(withdrawn.graph.pending_authority == 0);
        assert!(withdrawn.chat.pending_authority == 0);
        // The facts stay retained and attributable: revocation withholds them
        // from the projection, it does not erase them.
        assert_eq!(withdrawn.graph.revoked_authority, 2);
        assert_eq!(withdrawn.chat.revoked_authority, 3);
        assert_eq!(withdrawn.moot.delegated_certificates, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn worker_releases_files_before_reopen_and_advances_generation() {
        let root =
            std::env::temp_dir().join(format!("turnstone-place-release-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let original = root.join("original");
        let moved = root.join("moved");
        let identity = Arc::new(RootIdentity::Unsealed(InMemoryProvider::from_seed(
            [0x91; 32],
        )));
        let binding = binding(0x41);
        seed_profile(&original, identity.as_ref(), &binding, 1);

        let wake: Wake = Arc::new(|| {});
        let (worker, updates) = spawn_place_worker(wake, identity, settings());
        worker.command(PlaceWorkerCommand::Open {
            session: SessionId::new(),
            generation: 1,
            directory: original.clone(),
            binding: binding.clone(),
        });
        let first = updates
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(matches!(
            first,
            Update::PlaceOpened {
                generation: 1,
                result: Ok(_),
                ..
            }
        ));

        let (ack_tx, ack_rx) = std::sync::mpsc::sync_channel(1);
        worker.command(PlaceWorkerCommand::Release(ack_tx));
        ack_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        std::fs::rename(&original, &moved)
            .expect("release acknowledgement means the session directory can move");

        worker.command(PlaceWorkerCommand::Open {
            session: SessionId::new(),
            generation: 2,
            directory: moved,
            binding,
        });
        let second = updates
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(matches!(
            second,
            Update::PlaceOpened {
                generation: 2,
                result: Ok(_),
                ..
            }
        ));
        let (ack_tx, ack_rx) = std::sync::mpsc::sync_channel(1);
        worker.command(PlaceWorkerCommand::Release(ack_tx));
        ack_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }
}
