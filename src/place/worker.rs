//! Shell-owned retained-place worker.
//!
//! This is the product composition boundary. It opens Gemot, Commons graph,
//! Commons chat, and Stickleback group state for one Turnstone session, then
//! emits only app-owned summaries tagged with the session and open generation.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use armillary::{ActorHandle, Emitter, Wake, spawn_named};
use commons::Replica;
use commons::chat::ChatReplica;
use gemot::moot::{
    AvailabilityPolicy, ErasurePolicy, KeepBound, MootFile, MootId, MootRetentionSettings,
    PolicyRevision,
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

/// Runtime settings for the offline worker. The default is deliberately
/// conservative: opening cached state never proposes expiry or erasure.
#[derive(Clone, Debug)]
pub struct PlaceWorkerSettings {
    pub retention: MootRetentionSettings,
}

impl Default for PlaceWorkerSettings {
    fn default() -> Self {
        Self {
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

    let graph_backend = RedbBackend::open(stores.join("commons-graph.redb"))
        .map_err(|error| format!("open Commons graph cache: {error}"))?;
    let graph = Replica::for_identity(graph_backend, binding.root.0, identity)
        .map_err(|error| format!("bind Commons graph writer: {error}"))?;
    let graph_projection = pollster::block_on(graph.projection())
        .map_err(|error| format!("materialize Commons graph: {error}"))?;

    let chat_backend = RedbBackend::open(stores.join("commons-chat.redb"))
        .map_err(|error| format!("open Commons chat cache: {error}"))?;
    let chat = ChatReplica::for_identity(chat_backend, binding.chat.0, identity, keyring)
        .map_err(|error| format!("bind Commons chat writer: {error}"))?;
    let chat_projection = pollster::block_on(chat.projection())
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
    use gemot::moot::constitution::ConstitutionRules;
    use identity::InMemoryProvider;

    use crate::place::{ChatSpaceId, PlaceId, SharedContainerId};

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
        let founder = InMemoryProvider::from_seed([binding.moot.0[0].wrapping_add(40); 32]);
        let founder_id = founder.master_public_key().to_bytes();
        let settings = PlaceWorkerSettings::default();
        let stores = place_store_dir(directory);
        std::fs::create_dir_all(&stores).unwrap();
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
            ConstitutionRules::founder_only(founder_id),
            1,
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

        let (_, first) = open_cached_place(
            &first_dir,
            &first_binding,
            &first_identity,
            &PlaceWorkerSettings::default(),
        )
        .unwrap();
        let (_, second) = open_cached_place(
            &second_dir,
            &second_binding,
            &second_identity,
            &PlaceWorkerSettings::default(),
        )
        .unwrap();
        assert_eq!((first.graph.nodes, first.chat.messages), (1, 1));
        assert_eq!((second.graph.nodes, second.chat.messages), (2, 2));
        assert!(first.group.has_current_epoch);
        assert!(second.group.has_current_epoch);
        assert_eq!(first.moot.delegated_certificates, 0);
        assert_eq!(second.moot.delegated_certificates, 0);
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
        let (worker, updates) = spawn_place_worker(wake, identity, PlaceWorkerSettings::default());
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
