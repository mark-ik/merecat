//! The retained private-Knot publishing service behind Turnstone's panel.
//!
//! The Cambium pane owns text fields and local selection. This worker owns the
//! unlocked source handle, private tickets, revocation ledger, and carrier.
//! It starts only for a persona-vault Knot host and uses active mDNS without a
//! relay URL; an exported ticket remains the deliberate off-LAN fallback.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use base64::Engine as _;
use identity::IdentityProvider;
use knot::{
    KnotPublishCandidate, KnotPublishEligibility, KnotPublishHostLimits, KnotPublishSource,
    KnotShareRecipient, NetworkId, ProfileRef, PublicationId, TrustedRoot, publish_alpn,
    publish_policy, revoke_share,
};
use tokio::sync::mpsc as tokio_mpsc;
use transport::Transport;
use transport::p2panda_transport::{MdnsDiscoveryMode, P2pandaTransport};

use crate::identity::RootIdentity;

const NETWORK_CONTEXT: &str = "turnstone.knot-publish.network.v1";
const AUTHORITY_CONTEXT: &str = "turnstone.knot-publish.authority.v1";
const PROFILE_ID: &str = "mere.knot.publish";
const PROFILE_REVISION: u32 = 1;
const DEFAULT_SHARE_HOURS: u64 = 24;

/// The owner-visible status of one source candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishCandidateView {
    pub source_document: String,
    pub title: String,
    pub media_type: String,
    pub head: Option<[u8; 32]>,
    pub eligibility: KnotPublishEligibility,
    pub publication: Option<PublicationId>,
}

/// One share the owner can still revoke. The raw ticket never appears here;
/// only the most recently issued handoff is exposed once for the local owner
/// to copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishShareView {
    pub id: u64,
    pub publication: PublicationId,
    pub reader: [u8; 32],
    pub expires_at_ms: Option<u64>,
    pub revoked: bool,
}

/// Atomically readable control-plane state for the retained Cambium pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishSnapshot {
    pub status: String,
    pub route: String,
    pub candidates: Vec<PublishCandidateView>,
    pub shares: Vec<PublishShareView>,
    pub selected: Option<PublicationId>,
    pub latest_share: Option<u64>,
    pub latest_ticket: Option<String>,
}

impl Default for PublishSnapshot {
    fn default() -> Self {
        Self {
            status: "Knot publishing is unavailable: configure a persona-vault Knot host.".into(),
            route: "mDNS direct LAN preferred; no relay configured".into(),
            candidates: Vec::new(),
            shares: Vec::new(),
            selected: None,
            latest_share: None,
            latest_ticket: None,
        }
    }
}

enum PublishCommand {
    Refresh,
    SelectSource(String),
    Unpublish(PublicationId),
    Issue {
        reader: [u8; 32],
        expires_at_ms: Option<u64>,
    },
    Revoke(u64),
}

struct IssuedShare {
    ticket: knot::KnotShareTicket,
    reader: [u8; 32],
    expires_at_ms: Option<u64>,
    revoked: bool,
}

/// A shell-owned service handle. Dropping the final handle closes its command
/// channel and stops the private carrier with the worker thread.
pub struct KnotPublishingService {
    commands: tokio_mpsc::UnboundedSender<PublishCommand>,
    snapshot: Arc<Mutex<PublishSnapshot>>,
}

impl KnotPublishingService {
    /// Start a publishing carrier from the same startup unlock that created
    /// the authoring endpoint. The service has no relay URL and activates
    /// active p2panda mDNS for the preferred same-LAN route.
    pub fn start(source: KnotPublishSource, identity: Arc<RootIdentity>) -> Result<Self, String> {
        let (commands, receiver) = tokio_mpsc::unbounded_channel();
        let snapshot = Arc::new(Mutex::new(PublishSnapshot::default()));
        let (ready_send, ready_receive) = mpsc::sync_channel(1);
        let worker_snapshot = Arc::clone(&snapshot);
        std::thread::Builder::new()
            .name("turnstone-knot-publishing".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = ready_send.send(Err(format!("create publishing runtime: {error}")));
                        return;
                    }
                };
                if let Err(error) =
                    runtime.block_on(run(source, identity, receiver, worker_snapshot, ready_send))
                {
                    tracing::warn!(%error, "Knot publishing worker stopped");
                }
            })
            .map_err(|error| format!("start Knot publishing worker: {error}"))?;
        ready_receive
            .recv()
            .map_err(|_| "Knot publishing worker stopped during startup".to_string())??;
        Ok(Self { commands, snapshot })
    }

    pub fn snapshot(&self) -> PublishSnapshot {
        self.snapshot
            .lock()
            .expect("publishing snapshot poisoned")
            .clone()
    }

    pub fn refresh(&self) {
        let _ = self.commands.send(PublishCommand::Refresh);
    }

    pub fn select_source(&self, source_document: String) {
        let _ = self
            .commands
            .send(PublishCommand::SelectSource(source_document));
    }

    pub fn unpublish(&self, publication: PublicationId) {
        let _ = self.commands.send(PublishCommand::Unpublish(publication));
    }

    pub fn issue_selected(&self, reader: [u8; 32], expires_at_ms: Option<u64>) {
        let _ = self.commands.send(PublishCommand::Issue {
            reader,
            expires_at_ms,
        });
    }

    pub fn revoke(&self, share: u64) {
        let _ = self.commands.send(PublishCommand::Revoke(share));
    }

    pub const fn default_share_hours() -> u64 {
        DEFAULT_SHARE_HOURS
    }
}

async fn run(
    source: KnotPublishSource,
    identity: Arc<RootIdentity>,
    mut commands: tokio_mpsc::UnboundedReceiver<PublishCommand>,
    snapshot: Arc<Mutex<PublishSnapshot>>,
    ready: mpsc::SyncSender<Result<(), String>>,
) -> Result<(), String> {
    let root = identity.master_public_key().to_bytes();
    let network = NetworkId(blake3::derive_key(NETWORK_CONTEXT, &root));
    let root_authority = blake3::derive_key(AUTHORITY_CONTEXT, &root);
    let carrier_seed = source.transport_seed();
    let carrier = Arc::new(
        P2pandaTransport::builder_from_seed(carrier_seed)
            .alpns(vec![publish_alpn()])
            .mdns(MdnsDiscoveryMode::Active)
            .bind()
            .await
            .map_err(|error| format!("bind Knot publishing carrier: {error}"))?,
    );
    if carrier.local_peer_id().to_bytes() != source.publisher() {
        let error = "Knot publishing carrier identity differs from its source host".to_string();
        let _ = ready.send(Err(error.clone()));
        return Err(error);
    }
    let policy = publish_policy(
        network,
        vec![TrustedRoot {
            authority: root_authority,
            issuer: root,
        }],
        vec![ProfileRef {
            id: PROFILE_ID.into(),
            revision: PROFILE_REVISION,
        }],
        Some(8),
    );
    let host = source.into_host(
        policy,
        knot::KnotPublishCatalog::default(),
        KnotPublishHostLimits::default(),
    );
    refresh(&host, &snapshot, &BTreeMap::new()).await;
    set_status(
        &snapshot,
        "Ready. Select one eligible retained document before sharing.",
    );
    let _ = ready.send(Ok(()));

    // Accepting a reader is independent from an owner pressing a panel
    // control. Keeping this loop alive avoids cancelling a stream merely
    // because the owner refreshed, issued, or revoked a share.
    let (serve_outcomes_send, mut serve_outcomes) = tokio_mpsc::unbounded_channel();
    let serving_host = host.clone();
    let serving_carrier = Arc::clone(&carrier);
    tokio::spawn(async move {
        loop {
            let outcome = serving_host
                .accept_and_serve(serving_carrier.as_ref())
                .await;
            let retry_after_error = outcome.is_err();
            if serve_outcomes_send.send(outcome).is_err() {
                return;
            }
            if retry_after_error {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    });

    let mut selected_by_source = BTreeMap::<String, PublicationId>::new();
    let mut issued = BTreeMap::<u64, IssuedShare>::new();
    let mut next_share = 1_u64;
    loop {
        tokio::select! {
            biased;
            command = commands.recv() => {
                let Some(command) = command else { return Ok(()); };
                match command {
                    PublishCommand::Refresh => refresh(&host, &snapshot, &selected_by_source).await,
                    PublishCommand::SelectSource(source_document) => {
                        let candidates = match host.candidates().await {
                            Ok(candidates) => candidates,
                            Err(error) => {
                                set_status(&snapshot, format!("Could not inspect retained sources: {error}"));
                                continue;
                            }
                        };
                        let selected = candidates.into_iter().find(|candidate| {
                            candidate.source_document == source_document
                                && candidate.eligibility == KnotPublishEligibility::Eligible
                        });
                        match selected {
                            Some(candidate) => {
                                let publication = host.publish(candidate.source_document.clone()).await;
                                selected_by_source.insert(candidate.source_document, publication);
                                snapshot.lock().expect("publishing snapshot poisoned").selected = Some(publication);
                                set_status(&snapshot, format!("Selected publication {}", publication.as_uuid()));
                                refresh(&host, &snapshot, &selected_by_source).await;
                            }
                            None => set_status(&snapshot, "That source is absent or not currently eligible for publication."),
                        }
                    }
                    PublishCommand::Unpublish(publication) => {
                        if host.unpublish(publication).await {
                            selected_by_source.retain(|_, selected| *selected != publication);
                            set_status(&snapshot, "Publication withdrawn. Current and retained reads stop through this host.");
                            refresh(&host, &snapshot, &selected_by_source).await;
                        } else {
                            set_status(&snapshot, "That publication is not selected by this host.");
                        }
                    }
                    PublishCommand::Issue { reader, expires_at_ms } => {
                        let selected = snapshot.lock().expect("publishing snapshot poisoned").selected;
                        let Some(publication) = selected else {
                            set_status(&snapshot, "Select an eligible source before issuing a share.");
                            continue;
                        };
                        let endpoint_ticket = match carrier.ticket().await {
                            Ok(ticket) => ticket,
                            Err(error) => {
                                set_status(&snapshot, format!("Could not create the ticket endpoint fallback: {error}"));
                                continue;
                            }
                        };
                        let now = now_ms();
                        let ticket = host.issue_share(
                            identity.as_ref(),
                            KnotShareRecipient {
                                publication,
                                publisher: [0; 32],
                                reader,
                                network,
                                endpoint_ticket,
                                root_authority,
                                issued_at_ms: now,
                                expires_at_ms,
                                pinned_head: None,
                            },
                        ).await;
                        match ticket {
                            Ok(ticket) => {
                                let handoff = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
                                    serde_json::to_vec(&ticket)
                                        .map_err(|error| format!("encode publishing ticket: {error}"))?,
                                );
                                let share = next_share;
                                next_share = next_share.saturating_add(1);
                                issued.insert(share, IssuedShare { ticket, reader, expires_at_ms, revoked: false });
                                update_shares(&snapshot, &issued, Some(share), Some(handoff));
                                set_status(&snapshot, "Reader-bound share issued. Copy the ticket only through your chosen private handoff.");
                            }
                            Err(error) => set_status(&snapshot, format!("Could not issue share: {error}")),
                        }
                    }
                    PublishCommand::Revoke(share) => {
                        let Some(issued_share) = issued.get(&share) else {
                            set_status(&snapshot, "That share is no longer retained by this host.");
                            continue;
                        };
                        if issued_share.revoked {
                            set_status(&snapshot, "That share is already revoked.");
                            continue;
                        }
                        let ticket = issued_share.ticket.clone();
                        match revoke_share(identity.as_ref(), &ticket, now_ms()) {
                            Ok(revocation) => {
                                if host.revocations().write().await.fold(&revocation) {
                                    if let Some(issued_share) = issued.get_mut(&share) {
                                        issued_share.revoked = true;
                                    }
                                    update_shares(&snapshot, &issued, Some(share), None);
                                    set_status(&snapshot, "Share revoked. A later request with the same ticket is refused.");
                                } else {
                                    set_status(&snapshot, "The host could not retain its signed revocation.");
                                }
                            }
                            Err(error) => set_status(&snapshot, format!("Could not revoke share: {error}")),
                        }
                    }
                }
            }
            outcome = serve_outcomes.recv() => {
                let Some(outcome) = outcome else {
                    return Err("Knot publishing accept loop ended unexpectedly".into());
                };
                match outcome {
                    Ok(knot::KnotPublishServeOutcome::Responded) => set_status(&snapshot, "Served one admitted reader request."),
                    Ok(knot::KnotPublishServeOutcome::Refused(_)) | Ok(knot::KnotPublishServeOutcome::Lapsed(_)) => {
                        set_status(&snapshot, "Refused a reader before source disclosure.");
                    }
                    Err(error) => set_status(&snapshot, format!("Publishing carrier error: {error}")),
                }
            }
        }
    }
}

async fn refresh(
    host: &knot::KnotPublishHost<muniment::RedbBackend>,
    snapshot: &Arc<Mutex<PublishSnapshot>>,
    selected_by_source: &BTreeMap<String, PublicationId>,
) {
    match host.candidates().await {
        Ok(candidates) => {
            let selected = snapshot
                .lock()
                .expect("publishing snapshot poisoned")
                .selected;
            let candidates = candidates
                .into_iter()
                .map(|candidate| candidate_view(candidate, selected_by_source))
                .collect();
            let mut current = snapshot.lock().expect("publishing snapshot poisoned");
            current.candidates = candidates;
            current.selected = selected
                .filter(|publication| selected_by_source.values().any(|id| id == publication));
        }
        Err(error) => set_status(
            snapshot,
            format!("Could not inspect retained sources: {error}"),
        ),
    }
}

fn candidate_view(
    candidate: KnotPublishCandidate,
    selected_by_source: &BTreeMap<String, PublicationId>,
) -> PublishCandidateView {
    PublishCandidateView {
        publication: selected_by_source.get(&candidate.source_document).copied(),
        source_document: candidate.source_document,
        title: candidate.title,
        media_type: candidate.media_type,
        head: candidate.head,
        eligibility: candidate.eligibility,
    }
}

fn update_shares(
    snapshot: &Arc<Mutex<PublishSnapshot>>,
    issued: &BTreeMap<u64, IssuedShare>,
    latest_share: Option<u64>,
    latest_ticket: Option<String>,
) {
    let mut current = snapshot.lock().expect("publishing snapshot poisoned");
    current.shares = issued
        .iter()
        .map(|(id, share)| PublishShareView {
            id: *id,
            publication: share.ticket.publication,
            reader: share.reader,
            expires_at_ms: share.expires_at_ms,
            revoked: share.revoked,
        })
        .collect();
    current.latest_share = latest_share;
    current.latest_ticket = latest_ticket;
}

fn set_status(snapshot: &Arc<Mutex<PublishSnapshot>>, status: impl Into<String>) {
    snapshot
        .lock()
        .expect("publishing snapshot poisoned")
        .status = status.into();
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
