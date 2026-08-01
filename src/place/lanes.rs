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

/// How often the watcher samples lane counters, and how long they must hold
/// steady before it reports. A burst of operations from one sync round then
/// settles into a single re-fold instead of one per message.
const WATCH_TICK: std::time::Duration = std::time::Duration::from_millis(250);

/// One place's joined lanes, plus the transport and runtime that carry them.
///
/// Field order is drop order and is load-bearing: the watcher stops first,
/// then lane tasks abort, then the transport's actors stop, then the runtime
/// they all lived on shuts down. The runtime last, because aborting a task
/// needs a live runtime.
pub(crate) struct LiveLanes {
    watcher: Option<tokio::task::JoinHandle<()>>,
    moot: MootLanes,
    graph: JoinedSpace<CommonsExt>,
    chat: JoinedSpace<ChatExt>,
    _transport: P2pandaTransport,
    _runtime: tokio::runtime::Runtime,
}

impl Drop for LiveLanes {
    fn drop(&mut self) {
        // Explicit because the watcher outlives nothing else usefully: it
        // holds an Emitter, and a tick landing after the place closed would
        // ask the app to resync a place that is gone.
        if let Some(watcher) = self.watcher.take() {
            watcher.abort();
        }
    }
}

/// Shared counter handles across all seven lanes, sampled by the watcher.
struct LaneCounters {
    handles: Vec<std::sync::Arc<std::sync::Mutex<stickleback::SyncStatus>>>,
}

impl LaneCounters {
    /// Accepted operations across every lane. A sum is right HERE and wrong in
    /// the status surface: the watcher only needs to know that something
    /// arrived, while a person needs to know which lane it arrived on.
    fn total(&self) -> u64 {
        self.handles
            .iter()
            .map(|handle| handle.lock().map(|status| status.ops_received).unwrap_or(0))
            .sum()
    }
}

impl LiveLanes {
    fn counter_handles(&self) -> LaneCounters {
        let mut handles = self.moot.status_handles().to_vec();
        handles.push(self.graph.status_handle());
        handles.push(self.chat.status_handle());
        LaneCounters { handles }
    }

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


    /// Push one freshly authored graph operation onto the live lane.
    ///
    /// Initial sync covers retained history for a late joiner; an operation
    /// authored while live reaches connected peers only through this. Storing
    /// it is what makes it survive, publishing is what makes it arrive.
    pub(crate) fn publish_graph(
        &self,
        operation: stickleback::Operation<CommonsExt>,
    ) -> Result<(), String> {
        self.graph
            .publish(operation)
            .map_err(|error| format!("publish graph operation: {error}"))
    }

    /// Push one freshly authored chat operation onto the live lane.
    pub(crate) fn publish_chat(
        &self,
        operation: stickleback::Operation<ChatExt>,
    ) -> Result<(), String> {
        self.chat
            .publish(operation)
            .map_err(|error| format!("publish chat operation: {error}"))
    }
}

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
    use crate::place::PlaceBindingV1;
    use crate::place::invite::{P2PANDA_ENDPOINT_TICKET, RendezvousV1};
    use crate::place::worker::tests::{
        binding, found_place_for_authoring, founder_signing_key, place_delegation, place_rules,
        settings,
    };
    use crate::place::worker::{
        PlaceCommand, PlaceWorkerCommand, author_invitation, found_place_group,
        load_group_session, open_cached_place, place_store_dir, prepare_group_identity,
        spawn_place_worker,
    };
    use commons::{Replica, chat::ChatReplica};

    /// Issue a capability delegation on the host's retained delegation lane.
    fn delegate_to(host: &Path, founder: &InMemoryProvider, moot: [u8; 32], subject: [u8; 32]) {
        let rules = place_rules(founder.master_public_key().to_bytes());
        let moot_file = pollster::block_on(gemot::moot::MootFile::open_existing(
            place_store_dir(host).join("gemot"),
            gemot::moot::MootId(moot),
            settings().retention,
        ))
        .unwrap();
        pollster::block_on(moot_file.delegation_store().author_issue(
            &founder_signing_key(founder, moot),
            &rules,
            place_delegation(founder, moot, subject),
        ))
        .unwrap();
    }

    /// Author retained graph and chat content on the host as the founder.
    fn author_host_content(host: &Path, founder: &InMemoryProvider, b: &PlaceBindingV1) {
        let moot = b.moot.0;
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
            // Address as identity, matching what ShareNode authors: the
            // fixture must produce what the product path produces, or the
            // projection is proved against something nobody writes.
            pollster::block_on(graph.edit(move |log| {
                let address = format!("shared-{index}");
                log.insert_node(
                    &Author::new("turnstone"),
                    Container::new(address.clone()).with_address(address),
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
        let joiner_root = joiner.master_public_key().to_bytes();
        found_place_for_authoring(&host, &b, &founder, joiner_root);
        found_place_group(&host, &founder, b.moot.0).unwrap();
        author_host_content(&host, &founder, &b);
        // The joiner is a member AND delegated, so it can speak once the
        // delegation lane carries the certificate to it. Membership alone
        // would leave it able to read and not to write.
        delegate_to(&host, &founder, b.moot.0, joiner_root);
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

        // Catch-up WITHOUT polling Resync: the lane watcher nudges, the app
        // answers by resyncing, and the joiner converges on its own. This
        // stands in for the app's fold, which turns PlaceLanesAdvanced into an
        // Effect::ResyncPlace.
        let deadline = Instant::now() + Duration::from_secs(45);
        let mut last = None;
        let mut nudges = 0;
        loop {
            assert!(
                Instant::now() < deadline,
                "did not converge after {nudges} lane nudges; last: {last:?}"
            );
            match updates.recv_timeout(Duration::from_secs(10)) {
                Ok(Update::PlaceLanesAdvanced { generation: 1, .. }) => {
                    nudges += 1;
                    worker.command(PlaceWorkerCommand::Resync {
                        session,
                        generation: 1,
                    });
                }
                Ok(Update::PlaceOpened {
                    result: Ok(snapshot),
                    ..
                }) => {
                    // Content AND authority: the delegation lane must have
                    // carried the joiner's certificate too, or it converges on
                    // a place it can read and cannot speak in.
                    let done = snapshot.graph.nodes == 2
                        && snapshot.chat.messages == 2
                        && snapshot.chat.channels == 1
                        && snapshot.moot.members == 2
                        && snapshot.moot.delegated_certificates == 2;
                    last = Some(snapshot);
                    if done {
                        break;
                    }
                }
                _ => continue,
            }
        }
        assert!(
            nudges > 0,
            "the joiner converged without the watcher reporting anything"
        );

        // The joiner speaks, and its own projection carries the message back
        // immediately: authoring stores locally, publishing is what makes it
        // travel. Both halves ride the one command.
        worker.command(PlaceWorkerCommand::Author {
            session,
            generation: 1,
            request: 1,
            command: PlaceCommand::SendMessage {
                channel: "hall".into(),
                body: "from the joiner".into(),
            },
        });
        let authored = updates
            .recv_timeout(Duration::from_secs(30))
            .expect("author answers");
        match authored {
            Update::PlaceCommandDone {
                request: 1,
                result: Ok(snapshot),
                ..
            } => assert_eq!(
                snapshot.chat.messages, 3,
                "the author's own message projects at once"
            ),
            Update::PlaceCommandDone {
                result: Err(error), ..
            } => panic!("author refused: {error}"),
            _ => panic!("author answered with an unrelated update"),
        }

        // And it reaches the host over the live lane, which is the half
        // initial sync does not cover.
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            assert!(
                Instant::now() < deadline,
                "the authored message never reached the host"
            );
            std::thread::sleep(Duration::from_millis(400));
            let projection =
                pollster::block_on(host_open.chat.projection()).unwrap();
            if projection
                .messages
                .iter()
                .any(|message| message.message.body == "from the joiner")
            {
                break;
            }
        }

        // The shared graph reaches the app as nodes, not counts. Both of the
        // host's shared addresses are named, in deterministic order.
        let shared = last.expect("converged snapshot").shared;
        assert_eq!(shared.nodes.len(), 2);
        let addresses: Vec<&str> = shared.addresses().collect();
        assert_eq!(addresses, vec!["shared-0", "shared-1"]);

        let (ack_tx, ack_rx) = std::sync::mpsc::sync_channel(1);
        worker.command(PlaceWorkerCommand::Release(ack_tx));
        ack_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(_host_lanes);
        drop(host_open);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Wait for one authored command's answer, stepping past the lane nudges
    /// that arrive on the same channel. A bare `recv` races the watcher.
    fn expect_authored(updates: &std::sync::mpsc::Receiver<Update>, request: u64) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            match updates.recv_timeout(Duration::from_secs(10)) {
                Ok(Update::PlaceCommandDone {
                    request: got,
                    result: Ok(_),
                    ..
                }) if got == request => return,
                Ok(Update::PlaceCommandDone {
                    result: Err(error), ..
                }) => panic!("request {request} refused: {error}"),
                _ => continue,
            }
        }
        panic!("request {request} never answered");
    }

    /// Drive the worker until its snapshot satisfies `done`, answering lane
    /// nudges with resyncs. Returns the satisfying snapshot, or says how far
    /// it got.
    fn converge_until(
        worker: &armillary::ActorHandle<PlaceWorkerCommand>,
        updates: &std::sync::mpsc::Receiver<Update>,
        session: SessionId,
        what: &str,
        done: impl Fn(&crate::place::OfflinePlaceSnapshot) -> bool,
    ) -> crate::place::OfflinePlaceSnapshot {
        let deadline = Instant::now() + Duration::from_secs(45);
        let mut last = None;
        // Ask once up front: what is already retained may satisfy `done`
        // without any lane traffic at all.
        worker.command(PlaceWorkerCommand::Resync {
            session,
            generation: 1,
        });
        loop {
            assert!(Instant::now() < deadline, "{what}: never happened; {last:?}");
            match updates.recv_timeout(Duration::from_secs(10)) {
                Ok(Update::PlaceLanesAdvanced { .. }) => {
                    worker.command(PlaceWorkerCommand::Resync {
                        session,
                        generation: 1,
                    });
                }
                Ok(Update::PlaceOpened {
                    result: Ok(snapshot),
                    ..
                })
                | Ok(Update::PlaceCommandDone {
                    result: Ok(snapshot),
                    ..
                }) => {
                    if done(&snapshot) {
                        return snapshot;
                    }
                    last = Some(snapshot);
                    // No nudge is coming if the lanes are quiet, so keep
                    // asking rather than blocking on traffic that may not
                    // arrive.
                    std::thread::sleep(Duration::from_millis(300));
                    worker.command(PlaceWorkerCommand::Resync {
                        session,
                        generation: 1,
                    });
                }
                Ok(Update::PlaceCommandDone {
                    result: Err(error), ..
                }) => panic!("{what}: command refused: {error}"),
                _ => continue,
            }
        }
    }

    /// T3c's convergence half: a shared node crosses, a partition stops
    /// traffic without losing anything, and reconnecting converges both sides.
    #[test]
    fn a_partition_heals_and_both_sides_converge() {
        let root = std::env::temp_dir().join(format!(
            "turnstone-place-partition-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let host = root.join("host");
        let guest = root.join("guest");
        let founder = InMemoryProvider::from_seed([0xf1; 32]);
        let joiner = RootIdentity::Unsealed(InMemoryProvider::from_seed([0xf2; 32]));
        let b = binding(0x7e);
        let joiner_root = joiner.master_public_key().to_bytes();

        found_place_for_authoring(&host, &b, &founder, joiner_root);
        found_place_group(&host, &founder, b.moot.0).unwrap();
        author_host_content(&host, &founder, &b);
        delegate_to(&host, &founder, b.moot.0, joiner_root);
        let joiner_prekey = prepare_group_identity(&guest, &joiner, b.moot.0).unwrap();

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

        let (host_open, _) = open_cached_place(&host, &b, &founder, &settings()).unwrap();
        let (endpoint, gossip) = host_transport.sync_parts().unwrap();
        let host_lanes = runtime
            .block_on(async {
                let moot = host_open
                    .moot
                    .join_lanes(endpoint.clone(), gossip.clone())
                    .await?;
                let graph = host_open.graph.join(endpoint.clone(), gossip.clone()).await?;
                let chat = host_open.chat.join(endpoint, gossip).await?;
                Ok::<_, stickleback::JoinError>((moot, graph, chat))
            })
            .unwrap();

        let wake: armillary::Wake = Arc::new(|| {});
        let (worker, updates) = spawn_place_worker(wake, Arc::new(joiner), settings());
        let session = SessionId::new();
        worker.command(PlaceWorkerCommand::Join {
            session,
            generation: 1,
            directory: guest.clone(),
            invite: Box::new(invite),
        });
        assert!(matches!(
            updates.recv_timeout(Duration::from_secs(60)),
            Ok(Update::PlaceJoined { result: Ok(_), .. })
        ));
        converge_until(&worker, &updates, session, "initial catch-up", |s| {
            s.graph.nodes == 2 && s.moot.delegated_certificates == 2
        });

        // A shared HTTPS node authored by the joiner reaches the host.
        worker.command(PlaceWorkerCommand::Author {
            session,
            generation: 1,
            request: 1,
            command: PlaceCommand::ShareNode {
                address: "https://shared.example/page".into(),
            },
        });
        expect_authored(&updates, 1);
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            assert!(Instant::now() < deadline, "the shared node never crossed");
            std::thread::sleep(Duration::from_millis(300));
            let projection = pollster::block_on(host_open.graph.projection()).unwrap();
            if projection
                .graph
                .graph()
                .nodes()
                .any(|(_, c)| c.id == "https://shared.example/page")
            {
                break;
            }
        }

        // Partition: the host leaves its lanes. Nothing is lost on either
        // side, because authoring stores before it publishes.
        drop(host_lanes);
        worker.command(PlaceWorkerCommand::Author {
            session,
            generation: 1,
            request: 2,
            command: PlaceCommand::SendMessage {
                channel: "hall".into(),
                body: "sent while partitioned".into(),
            },
        });
        expect_authored(&updates, 2);

        // Heal: the host rejoins and catches up on what it missed.
        let (endpoint, gossip) = host_transport.sync_parts().unwrap();
        let _rejoined = runtime
            .block_on(async {
                let moot = host_open
                    .moot
                    .join_lanes(endpoint.clone(), gossip.clone())
                    .await?;
                let graph = host_open.graph.join(endpoint.clone(), gossip.clone()).await?;
                let chat = host_open.chat.join(endpoint, gossip).await?;
                Ok::<_, stickleback::JoinError>((moot, graph, chat))
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            assert!(
                Instant::now() < deadline,
                "the partitioned message never arrived after healing"
            );
            std::thread::sleep(Duration::from_millis(400));
            let projection = pollster::block_on(host_open.chat.projection()).unwrap();
            if projection
                .messages
                .iter()
                .any(|m| m.message.body == "sent while partitioned")
            {
                break;
            }
        }

        let (ack_tx, ack_rx) = std::sync::mpsc::sync_channel(1);
        worker.command(PlaceWorkerCommand::Release(ack_tx));
        ack_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(_rejoined);
        drop(host_open);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// T3c's restart half: what a place converged on survives the worker
    /// dying, and reopens without any network at all.
    ///
    /// It also pins the reconnection gap. `Open` does not dial: only `Join`
    /// carries a rendezvous, and the invitation is not persisted, so a
    /// restarted place comes back **offline** holding everything it had. That
    /// is correct behaviour for what exists and the wrong end state for a
    /// product; the assertion here is what will fail when reconnection lands,
    /// which is the point.
    #[test]
    fn a_restarted_place_keeps_what_it_converged_on() {
        let root = std::env::temp_dir().join(format!(
            "turnstone-place-restart-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let host = root.join("host");
        let guest = root.join("guest");
        let founder = InMemoryProvider::from_seed([0xa7; 32]);
        let joiner = RootIdentity::Unsealed(InMemoryProvider::from_seed([0xa8; 32]));
        let b = binding(0x8a);
        let joiner_root = joiner.master_public_key().to_bytes();

        found_place_for_authoring(&host, &b, &founder, joiner_root);
        found_place_group(&host, &founder, b.moot.0).unwrap();
        author_host_content(&host, &founder, &b);
        delegate_to(&host, &founder, b.moot.0, joiner_root);
        let joiner_prekey = prepare_group_identity(&guest, &joiner, b.moot.0).unwrap();

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

        let (host_open, _) = open_cached_place(&host, &b, &founder, &settings()).unwrap();
        let (endpoint, gossip) = host_transport.sync_parts().unwrap();
        let host_lanes = runtime
            .block_on(async {
                let moot = host_open
                    .moot
                    .join_lanes(endpoint.clone(), gossip.clone())
                    .await?;
                let graph = host_open.graph.join(endpoint.clone(), gossip.clone()).await?;
                let chat = host_open.chat.join(endpoint, gossip).await?;
                Ok::<_, stickleback::JoinError>((moot, graph, chat))
            })
            .unwrap();

        let identity = Arc::new(joiner);
        let wake: armillary::Wake = Arc::new(|| {});
        let (worker, updates) =
            spawn_place_worker(wake.clone(), Arc::clone(&identity), settings());
        let session = SessionId::new();
        worker.command(PlaceWorkerCommand::Join {
            session,
            generation: 1,
            directory: guest.clone(),
            invite: Box::new(invite),
        });
        assert!(matches!(
            updates.recv_timeout(Duration::from_secs(60)),
            Ok(Update::PlaceJoined { result: Ok(_), .. })
        ));
        let converged = converge_until(&worker, &updates, session, "catch-up", |s| {
            s.graph.nodes == 2 && s.chat.messages == 2
        });

        // The worker dies, taking every lane and handle with it.
        let (ack_tx, ack_rx) = std::sync::mpsc::sync_channel(1);
        worker.command(PlaceWorkerCommand::Release(ack_tx));
        ack_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(worker);
        drop(updates);
        drop(host_lanes);

        // A fresh worker reopens from the persisted binding alone.
        let (restarted, restarted_updates) =
            spawn_place_worker(wake, identity, settings());
        // Retried because a real restart is a new PROCESS: reopening the same
        // redb in-process can briefly race the previous handle's file lock
        // under load. That is an artifact of testing restart without
        // restarting, not a condition the product meets.
        let deadline = Instant::now() + Duration::from_secs(30);
        let reopened = loop {
            restarted.command(PlaceWorkerCommand::Open {
                session,
                generation: 2,
                directory: guest.clone(),
                binding: b.clone(),
            });
            match restarted_updates.recv_timeout(Duration::from_secs(15)) {
                Ok(Update::PlaceOpened {
                    result: Ok(snapshot),
                    ..
                }) => break snapshot,
                Ok(Update::PlaceOpened {
                    result: Err(error), ..
                }) => {
                    assert!(
                        Instant::now() < deadline && error.contains("Cannot acquire lock"),
                        "reopen failed: {error}"
                    );
                    std::thread::sleep(Duration::from_millis(500));
                }
                _ => continue,
            }
        };

        // Everything survived, including the authority that made it visible.
        assert_eq!(reopened.graph.nodes, converged.graph.nodes);
        assert_eq!(reopened.chat.messages, converged.chat.messages);
        assert_eq!(reopened.moot.members, converged.moot.members);
        assert_eq!(
            reopened.moot.delegated_certificates,
            converged.moot.delegated_certificates
        );
        assert_eq!(reopened.shared, converged.shared);
        assert!(reopened.group.has_current_epoch, "the sealed epoch reopened");

        // The reopened place has no lanes, because `Open` does not dial and no
        // rendezvous was persisted. It is still fully usable: authoring stores
        // locally and will publish whenever it next joins, which is the whole
        // reason those are separate steps.
        restarted.command(PlaceWorkerCommand::Author {
            session,
            generation: 2,
            request: 9,
            command: PlaceCommand::SendMessage {
                channel: "hall".into(),
                body: "authored after restart, offline".into(),
            },
        });
        match restarted_updates.recv_timeout(Duration::from_secs(30)) {
            Ok(Update::PlaceCommandDone {
                request: 9,
                result: Ok(snapshot),
                ..
            }) => assert_eq!(
                snapshot.chat.messages,
                converged.chat.messages + 1,
                "an offline place still authors"
            ),
            Ok(Update::PlaceCommandDone {
                result: Err(error), ..
            }) => panic!("offline authoring refused: {error}"),
            _ => panic!("author answered with an unrelated update"),
        }

        let (ack_tx, ack_rx) = std::sync::mpsc::sync_channel(1);
        restarted.command(PlaceWorkerCommand::Release(ack_tx));
        ack_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(host_open);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A profile with no effective capability is refused before it authors.
    /// The refusal is local: an operation nobody would project is not worth
    /// storing, and "you may not" is a better answer than silent filtering.
    #[test]
    fn an_unauthorized_profile_is_refused_before_it_authors() {
        let root = std::env::temp_dir().join(format!(
            "turnstone-place-unauthorized-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let host = root.join("host");
        let guest = root.join("guest");
        let founder = InMemoryProvider::from_seed([0xe1; 32]);
        let joiner = RootIdentity::Unsealed(InMemoryProvider::from_seed([0xe2; 32]));
        let b = binding(0x7c);

        // Membership without a delegation: the joiner belongs to the Moot but
        // holds no capability, which is exactly the pending case.
        found_place_for_authoring(&host, &b, &founder, joiner.master_public_key().to_bytes());
        found_place_group(&host, &founder, b.moot.0).unwrap();
        let joiner_prekey = prepare_group_identity(&guest, &joiner, b.moot.0).unwrap();
        let invite = author_invitation(
            &host,
            &b,
            &founder,
            &joiner_prekey,
            u64::MAX,
            Vec::new(),
            &settings(),
        )
        .unwrap();

        let wake: armillary::Wake = Arc::new(|| {});
        let (worker, updates) = spawn_place_worker(wake, Arc::new(joiner), settings());
        let session = SessionId::new();
        worker.command(PlaceWorkerCommand::Join {
            session,
            generation: 1,
            directory: guest,
            invite: Box::new(invite),
        });
        assert!(matches!(
            updates.recv_timeout(Duration::from_secs(30)),
            Ok(Update::PlaceJoined { result: Ok(_), .. })
        ));

        worker.command(PlaceWorkerCommand::Author {
            session,
            generation: 1,
            request: 1,
            command: PlaceCommand::SendMessage {
                channel: "hall".into(),
                body: "unauthorized".into(),
            },
        });
        match updates.recv_timeout(Duration::from_secs(30)) {
            Ok(Update::PlaceCommandDone {
                result: Err(error), ..
            }) => assert!(error.contains("no effective capability"), "{error}"),
            Ok(Update::PlaceCommandDone { result: Ok(_), .. }) => {
                panic!("an unauthorized profile must not author")
            }
            _ => panic!("author answered with an unrelated update"),
        }

        let (ack_tx, ack_rx) = std::sync::mpsc::sync_channel(1);
        worker.command(PlaceWorkerCommand::Release(ack_tx));
        ack_rx.recv_timeout(Duration::from_secs(5)).unwrap();
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
    watch: Option<(
        armillary::Emitter<crate::action::Update>,
        crate::panes::SessionId,
        u64,
    )>,
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

    let mut lanes = LiveLanes {
        watcher: None,
        moot: moot_lanes,
        graph,
        chat,
        _transport: transport,
        _runtime: runtime,
    };

    // The watcher turns lane arrivals into ONE app-visible nudge per settled
    // burst. It reports that something arrived; it never folds a projection
    // itself, because the authority filter belongs on the worker thread with
    // the stores, not on a sampling task.
    if let Some((out, session, generation)) = watch {
        let counters = lanes.counter_handles();
        lanes.watcher = Some(lanes._runtime.spawn(async move {
            let mut settled = counters.total();
            loop {
                tokio::time::sleep(WATCH_TICK).await;
                let now = counters.total();
                if now == settled {
                    continue;
                }
                // Wait for the burst to stop growing before reporting, so a
                // sync round of fifty operations is one re-fold and not fifty.
                tokio::time::sleep(WATCH_TICK).await;
                let after = counters.total();
                if after != now {
                    continue;
                }
                settled = after;
                out.emit(crate::action::Update::PlaceLanesAdvanced {
                    session,
                    generation,
                });
            }
        }));
    }
    Ok(lanes)
}
