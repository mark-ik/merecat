//! Turnstone's live-lane composition: dial a ticket, hold seven handles.
//!
//! The shell-owned counterpart to the domain join helpers. Everything here is
//! composition: the transport crate owns dialing and overlay tagging, Gemot
//! owns its five lanes, Commons owns graph and chat, and this module only
//! decides what a *place* joins and in what order it lets go. Session and
//! transport identity never become content authority; every accept closure
//! runs the owning domain's admission, and projections stay
//! authority-filtered exactly as they are offline.

use commons::CommonsExt;
use commons::chat::ChatExt;
use gemot::moot::MootLanes;
use identity::IdentityProvider;
use stickleback::JoinedSpace;
use transport::{P2pandaTransport, sync_overlay_topic};

use crate::place::PlaceBindingV1;
use crate::place::worker::OpenPlace;

/// One place's joined lanes, plus the transport and runtime that carry them.
///
/// Field order is drop order and is load-bearing: lane tasks abort first,
/// then the transport's actors stop, then the runtime they all lived on shuts
/// down. The runtime last, because aborting a task needs a live runtime.
pub(crate) struct LiveLanes {
    moot: MootLanes,
    graph: JoinedSpace<CommonsExt>,
    chat: JoinedSpace<ChatExt>,
    _transport: P2pandaTransport,
    _runtime: tokio::runtime::Runtime,
}

impl LiveLanes {
    /// Per-lane received/sent counters, Gemot's five then graph then chat,
    /// for the status surface: it must be able to say which lane is behind.
    pub(crate) fn ops_received(&self) -> [u64; 7] {
        let gemot = self.moot.sync_status();
        [
            gemot[0].ops_received,
            gemot[1].ops_received,
            gemot[2].ops_received,
            gemot[3].ops_received,
            gemot[4].ops_received,
            self.graph.sync_status().ops_received,
            self.chat.sync_status().ops_received,
        ]
    }

}

// T3b adds the publish half here: initial sync covers retained history for a
// late joiner, while operations authored WHILE live reach peers only through
// `JoinedSpace::publish` on the graph and chat handles above.

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use chartulary::{Author, Container};
    use commons::chat::{Channel, ChatEvent, Message};
    use identity::{IdentityProvider, InMemoryProvider};
    use muniment::RedbBackend;
    use stickleback::DataKeyring;
    use transport::P2pandaTransport;

    use crate::action::Update;
    use crate::identity::RootIdentity;
    use crate::panes::SessionId;
    use crate::place::invite::{P2PANDA_ENDPOINT_TICKET, RendezvousV1};
    use crate::place::worker::tests::{
        binding, found_place_for_authoring, founder_signing_key, place_delegation, place_rules,
        settings,
    };
    use crate::place::worker::{
        PlaceWorkerCommand, author_invitation, found_place_group, load_group_session,
        open_cached_place, place_store_dir, prepare_group_identity, spawn_place_worker,
    };
    use commons::{Replica, chat::ChatReplica};

    /// Author retained graph and chat content on the host as the founder.
    fn author_host_content(host: &Path, founder: &InMemoryProvider, moot: [u8; 32]) {
        let b = binding(0x7a);
        let stores = place_store_dir(host);
        // The founder's own writes must be Effective on the joiner, and a root
        // grant alone covers nothing: MootDelegations::covers walks
        // certificates only, so the founder delegates to itself.
        let rules = place_rules(founder.master_public_key().to_bytes());
        let moot_file = pollster::block_on(gemot::moot::MootFile::open_existing(
            stores.join("gemot"),
            gemot::moot::MootId(moot),
            settings().retention,
        ))
        .unwrap();
        pollster::block_on(moot_file.delegation_store().author_issue(
            &founder_signing_key(founder, moot),
            &rules,
            place_delegation(founder, moot, founder.master_public_key().to_bytes()),
        ))
        .unwrap();
        drop(moot_file);

        let group = load_group_session(host, founder, moot).unwrap();
        let keyring = DataKeyring::from_bytes(&group.data_keyring_state().unwrap()).unwrap();

        let graph_backend = RedbBackend::open(stores.join("commons-graph.redb")).unwrap();
        let mut graph = Replica::for_identity(graph_backend, b.root.0, founder).unwrap();
        for index in 0..2 {
            pollster::block_on(graph.edit(|log| {
                log.insert_node(
                    &Author::new("turnstone"),
                    Container::new(format!("shared-{index}")),
                );
            }))
            .unwrap();
        }
        drop(graph);

        let chat_backend = RedbBackend::open(stores.join("commons-chat.redb")).unwrap();
        let mut chat = ChatReplica::for_identity(chat_backend, b.chat.0, founder, keyring).unwrap();
        pollster::block_on(chat.author(ChatEvent::Channel(Channel {
            id: "hall".into(),
            title: "Hall".into(),
        })))
        .unwrap();
        for index in 0..2 {
            pollster::block_on(chat.author(ChatEvent::Message(Message {
                channel: "hall".into(),
                body: format!("retained {index}"),
                sent_at_ms: index as u64,
                reply_to: None,
            })))
            .unwrap();
        }
    }

    /// The T3a lane-join receipt: a joiner admits over a ticket and catches up
    /// on the founder's retained place, render-free, through the worker's own
    /// Join and Resync commands. Seven lanes per side, one endpoint each.
    #[test]
    fn a_joiner_catches_up_on_a_live_place_over_one_ticket() {
        let root = std::env::temp_dir().join(format!(
            "turnstone-place-live-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let host = root.join("host");
        let guest = root.join("guest");
        let founder = InMemoryProvider::from_seed([0xd1; 32]);
        let joiner = RootIdentity::Unsealed(InMemoryProvider::from_seed([0xd2; 32]));
        let b = binding(0x7a);

        // Retained state exists before anything dials.
        found_place_for_authoring(&host, &b, &founder, joiner.master_public_key().to_bytes());
        found_place_group(&host, &founder, b.moot.0).unwrap();
        author_host_content(&host, &founder, b.moot.0);
        let joiner_prekey = prepare_group_identity(&guest, &joiner, b.moot.0).unwrap();

        // The host binds its transport first, because the ticket in the
        // invitation IS this endpoint.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let host_transport = runtime
            .block_on(async {
                P2pandaTransport::builder(&founder.master_keypair())
                    .gossip()
                    .bind()
                    .await
            })
            .unwrap();
        let ticket = runtime.block_on(host_transport.ticket()).unwrap();

        let invite = author_invitation(
            &host,
            &b,
            &founder,
            &joiner_prekey,
            u64::MAX,
            vec![RendezvousV1 {
                carrier: P2PANDA_ENDPOINT_TICKET.into(),
                hint: ticket,
            }],
            &settings(),
        )
        .unwrap();

        // Host side goes live: the same open path the worker uses, lanes held
        // for the duration.
        let (host_open, _) = open_cached_place(&host, &b, &founder, &settings()).unwrap();
        let (endpoint, gossip) = host_transport.sync_parts().unwrap();
        let _host_lanes = runtime
            .block_on(async {
                let moot = host_open.moot.join_lanes(endpoint.clone(), gossip.clone()).await?;
                let graph = host_open.graph.join(endpoint.clone(), gossip.clone()).await?;
                let chat = host_open.chat.join(endpoint, gossip).await?;
                Ok::<_, stickleback::JoinError>((moot, graph, chat))
            })
            .unwrap();

        // Guest side is the product path end to end.
        let wake: armillary::Wake = Arc::new(|| {});
        let (worker, updates) =
            spawn_place_worker(wake, Arc::new(joiner), settings());
        let session = SessionId::new();
        worker.command(PlaceWorkerCommand::Join {
            session,
            generation: 1,
            directory: guest.clone(),
            invite: Box::new(invite),
        });
        let joined = updates
            .recv_timeout(Duration::from_secs(60))
            .expect("join answers");
        match joined {
            Update::PlaceJoined { result: Ok(_), .. } => {}
            Update::PlaceJoined {
                result: Err(error), ..
            } => panic!("join refused: {error}"),
            _ => panic!("join answered with an unrelated update"),
        }

        // Catch-up: poll Resync until the founder's retained state projects,
        // or say exactly how far it got.
        let deadline = Instant::now() + Duration::from_secs(45);
        let mut last = None;
        loop {
            assert!(
                Instant::now() < deadline,
                "did not converge; last snapshot: {last:?}"
            );
            std::thread::sleep(Duration::from_millis(400));
            worker.command(PlaceWorkerCommand::Resync {
                session,
                generation: 1,
            });
            let Ok(Update::PlaceOpened {
                result: Ok(snapshot),
                ..
            }) = updates.recv_timeout(Duration::from_secs(10))
            else {
                continue;
            };
            let done = snapshot.graph.nodes == 2
                && snapshot.chat.messages == 2
                && snapshot.chat.channels == 1
                && snapshot.moot.members == 2;
            last = Some(snapshot);
            if done {
                break;
            }
        }

        let (ack_tx, ack_rx) = std::sync::mpsc::sync_channel(1);
        worker.command(PlaceWorkerCommand::Release(ack_tx));
        ack_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(_host_lanes);
        drop(host_open);
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// Domain-separated salt for this place's transport identity. A derived key,
/// never the master: the transport peer id is session machinery, and the
/// worker's projections must stay exactly as valid if it changes.
fn transport_salt(moot: [u8; 32]) -> Vec<u8> {
    let mut salt = Vec::with_capacity(61);
    salt.extend_from_slice(b"turnstone.place.transport.v1/");
    salt.extend_from_slice(&moot);
    salt
}

/// Dial the invitation's tickets and join all seven of the place's lanes.
///
/// Takes the already-opened place rather than opening its own, so the lanes
/// drain into the same stores the worker's projections fold from. The
/// runtime is created here and owned by the returned value; the worker
/// thread stays synchronous.
pub(crate) fn join_live(
    open: &OpenPlace,
    binding: &PlaceBindingV1,
    identity: &dyn IdentityProvider,
    tickets: &[String],
) -> Result<LiveLanes, String> {
    if tickets.is_empty() {
        return Err("no dialable rendezvous".to_string());
    }
    let keypair = identity
        .derive_keypair(&transport_salt(binding.moot.0))
        .map_err(|error| format!("derive transport identity: {error}"))?;
    let overlays = [
        sync_overlay_topic(binding.moot.0),
        sync_overlay_topic(binding.root.0),
        sync_overlay_topic(binding.chat.0),
    ];

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|error| format!("build lane runtime: {error}"))?;

    let (transport, moot_lanes, graph, chat) = runtime.block_on(async {
        let transport = P2pandaTransport::builder(&keypair)
            .gossip()
            .bind()
            .await
            .map_err(|error| format!("bind place transport: {error}"))?;
        for ticket in tickets {
            let peer = transport
                .add_peer_ticket(ticket)
                .await
                .map_err(|error| format!("import rendezvous ticket: {error}"))?;
            transport
                .set_topics(peer, &overlays)
                .await
                .map_err(|error| format!("tag rendezvous overlays: {error}"))?;
        }
        let (endpoint, gossip) = transport
            .sync_parts()
            .ok_or_else(|| "place transport has no gossip".to_string())?;
        let moot_lanes = open
            .moot
            .join_lanes(endpoint.clone(), gossip.clone())
            .await
            .map_err(|error| format!("join Gemot lanes: {error}"))?;
        let graph = open
            .graph
            .join(endpoint.clone(), gossip.clone())
            .await
            .map_err(|error| format!("join graph lane: {error}"))?;
        let chat = open
            .chat
            .join(endpoint, gossip)
            .await
            .map_err(|error| format!("join chat lane: {error}"))?;
        Ok::<_, String>((transport, moot_lanes, graph, chat))
    })?;

    Ok(LiveLanes {
        moot: moot_lanes,
        graph,
        chat,
        _transport: transport,
        _runtime: runtime,
    })
}
