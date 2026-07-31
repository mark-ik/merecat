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
use stickleback::{
    DataKeyring, DropExportProfile, DropLimits, GroupControlFrame, GroupDirectFrame,
    GroupPrekeyBundle, GroupSession, GroupSessionId,
};

use crate::action::Update;
use crate::identity::RootIdentity;
use crate::panes::SessionId;
use crate::place::invite::PlaceInviteV1;
use crate::place::{
    ChatCache, GraphCache, GroupCache, MootCache, OfflinePlaceSnapshot, PlaceBindingV1,
};

const GROUP_SESSION_RECORD: &str = "group.session";
const GROUP_PREKEY_RECORD: &str = "group.prekey";

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
    Join {
        session: SessionId,
        generation: u64,
        directory: PathBuf,
        invite: Box<PlaceInviteV1>,
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

/// Bounds for a peer-supplied Gemot drop. An invitation arrives from someone
/// who is not yet trusted, so the reader is bounded before it allocates.
fn invite_drop_limits() -> DropLimits {
    DropLimits::default()
}

/// Admit one invitation, or leave nothing behind.
///
/// This is the gate the place-port plan's five checks describe. The ordering is
/// the point: every artifact is verified by its own domain first, and the
/// sealed group session is written only once all of them have answered. A
/// refused invitation must not leave a Gemot store, a sealed secret, or a
/// binding a later open could mistake for an admitted place.
///
/// Structural envelope validation is not admission and does not appear here
/// beyond its first line. Holding or forwarding an envelope grants nothing.
pub fn admit_invitation(
    directory: &Path,
    invite: &PlaceInviteV1,
    identity: &dyn IdentityProvider,
    settings: &PlaceWorkerSettings,
) -> Result<AdmittedPlace, String> {
    let store_existed = place_store_dir(directory).exists();
    match admit_inner(directory, invite, identity, settings) {
        Ok(admitted) => Ok(admitted),
        Err(error) => {
            // Refusal removes only what this attempt created. A half-imported
            // Gemot store would let a later open present an unadmitted place as
            // a retained one.
            //
            // The sealed secrets are deliberately not touched. They hold this
            // profile's own group identity, whose pre-key was published before
            // any invitation arrived; deleting it on a refused invitation would
            // let any stranger destroy the key material a pending welcome is
            // already addressed to.
            if !store_existed {
                let _ = std::fs::remove_dir_all(place_store_dir(directory));
            }
            Err(error)
        }
    }
}

/// Establish this profile's durable group identity for one Moot and return the
/// publishable pre-key bundle.
///
/// This must happen before an invitation can be issued, not after one arrives.
/// `GroupSession::new` draws its long-term key from the RNG, so the recipient
/// id is not derivable from the Personae root: the session that generated the
/// published pre-key is the only one a welcome can be addressed to. Creating a
/// fresh session at admission time would produce a different recipient and
/// refuse every genuine welcome.
///
/// Idempotent. A second call returns the same bundle rather than rotating the
/// identity out from under a welcome already in flight.
pub fn prepare_group_identity(
    directory: &Path,
    identity: &dyn IdentityProvider,
    moot: [u8; 32],
) -> Result<Vec<u8>, String> {
    let storage = sealed_group_store(directory, identity, moot)?;
    if let Some(bundle) = storage
        .load_record(GROUP_PREKEY_RECORD)
        .map_err(|error| format!("load sealed group pre-key: {error}"))?
    {
        return Ok(bundle);
    }
    let (session, prekey) = GroupSession::new(GroupSessionId(moot), identity)
        .map_err(|error| format!("create group session: {error}"))?;
    let bundle = prekey
        .to_bytes()
        .map_err(|error| format!("encode group pre-key: {error}"))?;
    save_group_session(directory, identity, &session)?;
    storage
        .save_record(GROUP_PREKEY_RECORD, &bundle)
        .map_err(|error| format!("seal group pre-key: {error}"))?;
    Ok(bundle)
}

/// Author an invitation to this profile's own place for one published pre-key.
///
/// The counterpart to [`admit_invitation`], and deliberately the same shape in
/// reverse: it reads only retained domain state, mints no authority, and every
/// field it fills is one the recipient re-derives and checks independently.
///
/// The caller supplies `joiner_prekey` out of band. Which channel carried it is
/// not this function's concern and must never become evidence: the bundle
/// carries its own Personae attestation, which is what binds it to a person.
pub fn author_invitation(
    directory: &Path,
    binding: &PlaceBindingV1,
    identity: &dyn IdentityProvider,
    joiner_prekey: &[u8],
    not_after_ms: u64,
    settings: &PlaceWorkerSettings,
) -> Result<PlaceInviteV1, String> {
    binding
        .validate()
        .map_err(|error| format!("place binding: {error}"))?;
    let prekey = GroupPrekeyBundle::from_bytes(joiner_prekey)
        .map_err(|error| format!("decode joiner pre-key: {error}"))?;
    if prekey.group != GroupSessionId(binding.moot.0) {
        return Err("joiner pre-key belongs to another group".to_string());
    }
    let joiner_root = prekey
        .personae_root()
        .map_err(|error| format!("verify joiner pre-key: {error}"))?;

    let stores = place_store_dir(directory);
    let moot = pollster::block_on(MootFile::open_existing(
        stores.join("gemot"),
        MootId(binding.moot.0),
        settings.retention.clone(),
    ))
    .map_err(|error| format!("open Gemot store: {error}"))?;
    let snapshot = pollster::block_on(moot.snapshot())
        .map_err(|error| format!("materialize Gemot: {error}"))?;
    // The recipient must already be a governed member. Inviting someone the
    // Moot has not admitted would mint an envelope that can only ever be
    // refused, and refusing here names the real reason instead.
    if !snapshot
        .membership
        .members
        .iter()
        .any(|member| member.member == joiner_root)
    {
        return Err("Gemot membership does not contain the invited root".to_string());
    }

    let mut evidence = Vec::new();
    pollster::block_on(moot.export_plain_drop(
        &mut evidence,
        DropExportProfile::default(),
        DropLimits::default(),
    ))
    .map_err(|error| format!("export Gemot evidence: {error}"))?;

    // Welcome the recipient into the crypto group, then persist: the epoch this
    // mints is the one the envelope names, so losing it would strand a welcome
    // this profile can no longer follow.
    let mut group = load_group_session(directory, identity, binding.moot.0)?;
    group
        .register_prekey(&prekey)
        .map_err(|error| format!("register joiner pre-key: {error}"))?;
    let dispatch = group
        .add(prekey.recipient)
        .map_err(|error| format!("welcome the joiner: {error}"))?;
    let direct = dispatch
        .direct_for(prekey.recipient)
        .ok_or_else(|| "welcome carries no frame for the invited recipient".to_string())?;
    let expected_epoch = group
        .current_epoch()
        .ok_or_else(|| "welcoming a member installed no epoch".to_string())?;
    save_group_session(directory, identity, &group)?;

    Ok(PlaceInviteV1 {
        version: crate::place::invite::PLACE_INVITE_VERSION,
        binding: binding.clone(),
        founder: snapshot.governance.founder,
        inviter: identity.master_public_key().to_bytes(),
        inviter_prekey: inline_artifact(
            &sealed_prekey_bytes(directory, identity, binding.moot.0)?,
        ),
        governance: inline_artifact(&evidence),
        key_welcome: inline_artifact(
            &dispatch
                .control
                .to_bytes()
                .map_err(|error| format!("encode welcome control: {error}"))?,
        ),
        key_direct: inline_artifact(
            &direct
                .to_bytes()
                .map_err(|error| format!("encode welcome frame: {error}"))?,
        ),
        expected_epoch,
        membership_heads: snapshot.membership.auth_heads,
        not_after_ms,
        rendezvous: Vec::new(),
    })
}

/// This profile's own published pre-key bundle for one Moot.
fn sealed_prekey_bytes(
    directory: &Path,
    identity: &dyn IdentityProvider,
    moot: [u8; 32],
) -> Result<Vec<u8>, String> {
    sealed_group_store(directory, identity, moot)?
        .load_record(GROUP_PREKEY_RECORD)
        .map_err(|error| format!("load sealed group pre-key: {error}"))?
        .ok_or_else(|| "this profile has no prepared group identity".to_string())
}

/// Wrap bytes as a digest-checked inline artifact.
fn inline_artifact(bytes: &[u8]) -> crate::place::invite::ArtifactRefV1 {
    crate::place::invite::ArtifactRefV1::Inline {
        media_type: "application/vnd.mere.place-artifact".into(),
        digest: *Digest::blake3(bytes)
            .bytes
            .first_chunk::<32>()
            .expect("blake3 produces 32 bytes"),
        bytes: bytes.to_vec(),
    }
}

/// Create this place's crypto group with this profile as its first member.
///
/// The founding counterpart to [`prepare_group_identity`]. A Moot's governance
/// fold and its DCGKA group are founded separately and neither implies the
/// other: Gemot decides who belongs, Stickleback decides who can read. Until
/// this runs, the founder holds a group identity but is not an active member
/// of any group, so it cannot welcome anyone.
///
/// Idempotent. A group that already has members is left exactly as it is
/// rather than re-created, since re-creating would strand every epoch already
/// handed out.
pub fn found_place_group(
    directory: &Path,
    identity: &dyn IdentityProvider,
    moot: [u8; 32],
) -> Result<(), String> {
    prepare_group_identity(directory, identity, moot)?;
    let mut group = load_group_session(directory, identity, moot)?;
    let members = group
        .members()
        .map_err(|error| format!("read group membership: {error}"))?;
    if !members.is_empty() {
        return Ok(());
    }
    group
        .create(&[])
        .map_err(|error| format!("create place group: {error}"))?;
    save_group_session(directory, identity, &group)
}

/// Reopen the sealed group session established by [`prepare_group_identity`].
fn load_group_session(
    directory: &Path,
    identity: &dyn IdentityProvider,
    moot: [u8; 32],
) -> Result<GroupSession, String> {
    let storage = sealed_group_store(directory, identity, moot)?;
    let bytes: Vec<u8> = storage
        .load_record(GROUP_SESSION_RECORD)
        .map_err(|error| format!("load sealed group session: {error}"))?
        .ok_or_else(|| "sealed group session is absent".to_string())?;
    GroupSession::from_bytes(&bytes)
        .map_err(|error| format!("decode sealed group session: {error}"))
}

/// One admitted place. Deliberately carries no store handle, key, or frame:
/// the caller persists the binding and reopens through the ordinary path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmittedPlace {
    pub binding: PlaceBindingV1,
    /// Governance membership: Gemot's converged fold.
    pub moot: MootCache,
    /// Crypto membership: the DCGKA group this welcome joined.
    ///
    /// Deliberately not the same number as `moot.members`, and not asserted
    /// equal to it. They are different folds and can legitimately disagree the
    /// moment a join completes, because the joiner has processed exactly one
    /// welcome while Gemot's evidence may already carry later membership. A
    /// lasting divergence is worth surfacing; an immediate one is normal.
    pub group_members: usize,
}

fn admit_inner(
    directory: &Path,
    invite: &PlaceInviteV1,
    identity: &dyn IdentityProvider,
    settings: &PlaceWorkerSettings,
) -> Result<AdmittedPlace, String> {
    invite
        .validate()
        .map_err(|error| format!("place invitation: {error}"))?;
    let binding = &invite.binding;
    let local_root = identity.master_public_key().to_bytes();

    // 0. The inviter's own time bound, checked before anything is created.
    //    The heads pin below already invalidates an invitation whenever the
    //    roster moves; this additionally stops a forwarded envelope working
    //    forever in a Moot whose membership never changes.
    let now_ms = settings.authority_clock.now_ms();
    if now_ms > invite.not_after_ms {
        return Err(format!(
            "invitation expired at {} and it is now {now_ms}",
            invite.not_after_ms
        ));
    }

    // 5. The Commons scopes are distinct governed roots, not aliases of the
    //    Moot or of each other. Checked first because it is free, and because
    //    a collision here would silently point two domains at one store.
    if binding.root.0 == binding.moot.0 || binding.chat.0 == binding.moot.0 {
        return Err("invitation reuses the Moot id as a Commons scope".to_string());
    }
    if binding.root.0 == binding.chat.0 {
        return Err("invitation gives the graph and chat the same scope".to_string());
    }

    // 1. The Gemot evidence addresses this Moot. Imported into the place's own
    //    store so the fold is Gemot's, not a claim the envelope makes.
    let stores = place_store_dir(directory);
    std::fs::create_dir_all(&stores)
        .map_err(|error| format!("create place store: {error}"))?;
    let moot = pollster::block_on(MootFile::open(
        stores.join("gemot"),
        MootId(binding.moot.0),
        invite.founder,
        settings.retention.clone(),
    ))
    .map_err(|error| format!("open Gemot store: {error}"))?;
    let evidence = invite
        .governance
        .verified_bytes("governance artifact")
        .map_err(|error| format!("place invitation: {error}"))?;
    let receipt = pollster::block_on(
        moot.import_plain_drop(std::io::Cursor::new(evidence), invite_drop_limits()),
    )
    .map_err(|error| format!("import Gemot evidence: {error}"))?;
    if receipt.snapshot.moot_id != MootId(binding.moot.0) {
        return Err("Gemot evidence addresses another Moot".to_string());
    }

    // 2. The converged membership fold contains the local Personae root, and
    //    the claimed inviter. The envelope names an author; Gemot decides
    //    whether that name is a member.
    let members = &receipt.snapshot.membership.members;
    if !members.iter().any(|member| member.member == local_root) {
        return Err("Gemot membership does not contain this Personae root".to_string());
    }
    if !members.iter().any(|member| member.member == invite.inviter) {
        return Err("invitation names an author outside Gemot membership".to_string());
    }

    // 3. The welcome is bound to this group and addressed to this recipient's
    //    authenticated crypto identity, and it produces a usable epoch.
    let control = GroupControlFrame::from_bytes(
        invite
            .key_welcome
            .verified_bytes("key welcome artifact")
            .map_err(|error| format!("place invitation: {error}"))?,
    )
    .map_err(|error| format!("decode group welcome: {error}"))?;
    let direct = GroupDirectFrame::from_bytes(
        invite
            .key_direct
            .verified_bytes("recipient welcome artifact")
            .map_err(|error| format!("place invitation: {error}"))?,
    )
    .map_err(|error| format!("decode recipient welcome: {error}"))?;
    let group_id = GroupSessionId(binding.moot.0);
    if control.group != group_id || direct.group != group_id {
        return Err("group welcome addresses another group".to_string());
    }
    if direct.control != control.id {
        return Err("recipient welcome belongs to another control frame".to_string());
    }
    // The group identity whose published pre-key this welcome answers. Absent
    // means nobody could have addressed a welcome to this profile yet.
    let mut group = load_group_session(directory, identity, binding.moot.0)
        .map_err(|error| format!("{error}; prepare a group identity before joining"))?;
    if direct.recipient != group.member() {
        return Err("group welcome is addressed to another recipient".to_string());
    }

    // The sender's authenticated pre-key. Its Personae attestation is what
    // makes `invite.inviter` a verified fact rather than an envelope claim, so
    // the two must agree before the bundle is registered.
    let inviter_prekey = GroupPrekeyBundle::from_bytes(
        invite
            .inviter_prekey
            .verified_bytes("inviter pre-key artifact")
            .map_err(|error| format!("place invitation: {error}"))?,
    )
    .map_err(|error| format!("decode inviter pre-key: {error}"))?;
    if inviter_prekey.group != group_id {
        return Err("inviter pre-key belongs to another group".to_string());
    }
    let attested = inviter_prekey
        .personae_root()
        .map_err(|error| format!("verify inviter pre-key: {error}"))?;
    if attested != invite.inviter {
        return Err("inviter pre-key attests a different Personae root".to_string());
    }
    group
        .register_prekey(&inviter_prekey)
        .map_err(|error| format!("register inviter pre-key: {error}"))?;
    group
        .process(invite.inviter, &control, Some(&direct))
        .map_err(|error| format!("process group welcome: {error}"))?;
    // 4. Gemot binds that epoch to the same membership heads.
    //
    // The epoch must be the one the invitation describes, and it must have been
    // minted against the membership state Gemot itself converged to from the
    // imported evidence. Without the second half, a welcome minted before a
    // removal would hand the joiner a key the departed member still holds, and
    // every other check would still pass.
    match group.current_epoch() {
        None => return Err("group welcome produced no current epoch".to_string()),
        Some(epoch) if epoch != invite.expected_epoch => {
            return Err("group welcome installed an epoch the invitation does not name".to_string());
        }
        Some(_) => {}
    }
    // `auth_heads()` returns sorted heads, and the envelope's are bounded and
    // compared as given: a reordered or padded list is a different claim.
    if receipt.snapshot.membership.auth_heads != invite.membership_heads {
        return Err(
            "invitation pins membership heads that Gemot did not converge to".to_string(),
        );
    }

    // Every domain has answered. Only now does anything durable exist, and the
    // binding is written last so the presence of `place.json` implies the whole
    // check list ran, not merely that an envelope parsed.
    save_group_session(directory, identity, &group)?;
    crate::session::save_place_binding(directory, binding)
        .map_err(|error| format!("persist place binding: {error}"))?;
    Ok(AdmittedPlace {
        binding: binding.clone(),
        moot: MootCache {
            membership_epoch: receipt.snapshot.membership.epoch,
            members: members.len(),
            roster_members: receipt.snapshot.roster.members.len(),
            delegated_certificates: receipt.snapshot.delegated_certificates,
            tessera_operations: receipt.snapshot.tessera_operations,
        },
        group_members: group
            .members()
            .map_err(|error| format!("materialize group membership: {error}"))?
            .len(),
    })
}

/// Persist the group session inside the same Personae-derived sealed boundary
/// the worker reopens. [`admit_invitation`] calls this after every check.
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
                    PlaceWorkerCommand::Join {
                        session,
                        generation,
                        directory,
                        invite,
                    } => {
                        // Admission first, then the ordinary cached open. The
                        // second step is not a formality: it proves the place
                        // admission just established actually reopens through
                        // the same path every later boot will use.
                        live = None;
                        let joined = admit_invitation(
                            &directory,
                            &invite,
                            identity.as_ref(),
                            &settings,
                        )
                        .and_then(|admitted| {
                            open_cached_place(
                                &directory,
                                &admitted.binding,
                                identity.as_ref(),
                                &settings,
                            )
                            .map(|(opened, snapshot)| (admitted.binding, opened, snapshot))
                        });
                        match joined {
                            Ok((binding, opened, snapshot)) => {
                                live = Some(opened);
                                out.emit(Update::PlaceJoined {
                                    session,
                                    generation,
                                    result: Ok((binding, snapshot)),
                                });
                            }
                            Err(error) => out.emit(Update::PlaceJoined {
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
    use gemot::moot::{
        MOOT_ACT_ACTION, MOOT_DELEGATION_DOMAIN, MootAccessLevel, MootMember,
        MootMembershipAction,
    };
    use stickleback::DropExportProfile;

    use crate::place::invite::{ArtifactRefV1, PLACE_INVITE_VERSION};
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

    /// Build a real invitation: a founded Moot whose membership contains both
    /// roots, exported as a plain drop, plus a genuine Stickleback welcome
    /// addressed to the joiner's registered pre-key.
    fn real_invitation(
        founder_dir: &Path,
        binding: &PlaceBindingV1,
        founder: &InMemoryProvider,
        joiner_published_prekey: Vec<u8>,
        joiner_id: [u8; 32],
    ) -> PlaceInviteV1 {
        let founder_id = founder.master_public_key().to_bytes();
        let stores = place_store_dir(founder_dir);
        std::fs::create_dir_all(&stores).unwrap();
        let moot = pollster::block_on(MootFile::open(
            stores.join("gemot"),
            MootId(binding.moot.0),
            founder_id,
            settings().retention,
        ))
        .unwrap();
        pollster::block_on(moot.found(
            founder.master_keypair().to_seed(),
            None,
            None,
            place_rules(founder_id),
            1,
        ))
        .unwrap();
        pollster::block_on(moot.membership_store().author_for_identity(
            founder,
            MootMembershipAction::Create {
                initial_members: vec![MootMember {
                    member: founder_id,
                    access: MootAccessLevel::Manage,
                }],
            },
        ))
        .unwrap();
        pollster::block_on(moot.membership_store().author_for_identity(
            founder,
            MootMembershipAction::Add {
                member: joiner_id,
                access: MootAccessLevel::Write,
            },
        ))
        .unwrap();

        let mut drop_bytes = Vec::new();
        pollster::block_on(moot.export_plain_drop(
            &mut drop_bytes,
            DropExportProfile::default(),
            DropLimits::default(),
        ))
        .unwrap();
        let membership_heads = pollster::block_on(moot.snapshot())
            .unwrap()
            .membership
            .auth_heads;
        drop(moot);

        // The joiner's group identity already exists and its pre-key is
        // published: that is the precondition for being invitable at all, since
        // the recipient id comes from the RNG rather than the Personae root.
        let joiner_prekey = GroupPrekeyBundle::from_bytes(&joiner_published_prekey).unwrap();
        let joiner_recipient = joiner_prekey.recipient;
        let (mut founder_group, founder_prekey) =
            GroupSession::new(GroupSessionId(binding.moot.0), founder).unwrap();
        founder_group.register_prekey(&joiner_prekey).unwrap();
        founder_group.create(&[]).unwrap();
        let dispatch = founder_group.add(joiner_recipient).unwrap();
        let direct = dispatch.direct_for(joiner_recipient).unwrap();

        PlaceInviteV1 {
            version: PLACE_INVITE_VERSION,
            binding: binding.clone(),
            founder: founder_id,
            inviter: founder_id,
            inviter_prekey: inline_artifact(&founder_prekey.to_bytes().unwrap()),
            governance: inline_artifact(&drop_bytes),
            key_welcome: inline_artifact(&dispatch.control.to_bytes().unwrap()),
            key_direct: inline_artifact(&direct.to_bytes().unwrap()),
            expected_epoch: founder_group
                .current_epoch()
                .expect("adding a member installs an epoch"),
            membership_heads,
            // Comfortably after the pinned AUTHORITY_AT_MS, so only the test
            // that moves the clock forward sees an expiry.
            not_after_ms: AUTHORITY_AT_MS + 1_000,
            rendezvous: Vec::new(),
        }
    }

    fn inline_artifact(bytes: &[u8]) -> ArtifactRefV1 {
        ArtifactRefV1::Inline {
            media_type: "application/vnd.mere.place-artifact".into(),
            digest: proofs::Digest::blake3(bytes)
                .bytes
                .as_slice()
                .try_into()
                .unwrap(),
            bytes: bytes.to_vec(),
        }
    }

    /// A joiner directory with a prepared identity, plus an invitation whose
    /// welcome is genuinely addressed to it.
    fn matched_case(
        root: &Path,
        name: &str,
        binding: &PlaceBindingV1,
        founder: &InMemoryProvider,
        joiner: &RootIdentity,
    ) -> (PathBuf, PlaceInviteV1) {
        let directory = root.join(name);
        let published = prepare_group_identity(&directory, joiner, binding.moot.0).unwrap();
        let invite = real_invitation(
            &root.join(format!("{name}-founder")),
            binding,
            founder,
            published,
            joiner.master_public_key().to_bytes(),
        );
        (directory, invite)
    }

    /// (Gemot store, sealed secrets, admitted binding).
    fn residue(directory: &Path) -> (bool, bool, bool) {
        (
            place_store_dir(directory).exists(),
            place_secrets_dir(directory).exists(),
            crate::session::place_binding_path(directory).exists(),
        )
    }

    #[test]
    fn a_valid_invitation_admits_and_a_refused_one_leaves_nothing_behind() {
        let root =
            std::env::temp_dir().join(format!("turnstone-place-admit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let founder = InMemoryProvider::from_seed([0xb1; 32]);
        let joiner = RootIdentity::Unsealed(InMemoryProvider::from_seed([0xb2; 32]));
        let stranger = RootIdentity::Unsealed(InMemoryProvider::from_seed([0xb3; 32]));
        let binding = binding(0x61);
        let joined = root.join("joiner");
        let joiner_id = joiner.master_public_key().to_bytes();

        // Being invitable is a precondition, not a consequence: the identity
        // exists and its pre-key is published before any envelope is authored.
        let published = prepare_group_identity(&joined, &joiner, binding.moot.0).unwrap();
        assert_eq!(
            prepare_group_identity(&joined, &joiner, binding.moot.0).unwrap(),
            published,
            "preparing twice must not rotate the identity a welcome is addressed to"
        );
        let invite = real_invitation(
            &root.join("founder"),
            &binding,
            &founder,
            published.clone(),
            joiner_id,
        );

        // The happy path: every domain answers, and only then is a secret sealed.
        let admitted =
            admit_invitation(&joined, &invite, &joiner, &settings()).expect("valid invitation");
        assert_eq!(admitted.binding, binding);
        assert_eq!(admitted.moot.members, 2, "Gemot's governance fold");
        assert_eq!(admitted.group_members, 2, "Stickleback's crypto fold");
        assert_eq!(residue(&joined), (true, true, true));

        // A tampered artifact never reaches a domain, and leaves no store.
        let tampered_dir = root.join("tampered");
        let mut tampered = invite.clone();
        let ArtifactRefV1::Inline { bytes, .. } = &mut tampered.governance else {
            unreachable!("fixture is inline")
        };
        bytes.push(0);
        let error =
            admit_invitation(&tampered_dir, &tampered, &joiner, &settings()).unwrap_err();
        assert!(error.contains("declared digest"), "{error}");
        assert_eq!(residue(&tampered_dir), (false, false, false));

        // A stranger holding the same envelope is refused by Gemot membership,
        // not by anything the envelope says about itself.
        let stranger_dir = root.join("stranger");
        let error =
            admit_invitation(&stranger_dir, &invite, &stranger, &settings()).unwrap_err();
        assert!(error.contains("membership does not contain"), "{error}");
        assert_eq!(residue(&stranger_dir), (false, false, false));

        // An envelope naming a non-member as its author is refused even though
        // the welcome frames themselves are genuine.
        let forged_dir = root.join("forged-author");
        let mut forged = invite.clone();
        forged.inviter = stranger.master_public_key().to_bytes();
        let error = admit_invitation(&forged_dir, &forged, &joiner, &settings()).unwrap_err();
        assert!(error.contains("outside Gemot membership"), "{error}");
        assert_eq!(residue(&forged_dir), (false, false, false));

        // Check 4's two refusals need a welcome genuinely addressed to the
        // directory under test, since every prepared identity draws a fresh
        // random recipient. Each gets its own matched invitation.
        //
        // An epoch other than the one the invitation names is refused even
        // though the welcome itself is genuine and processes cleanly.
        let (wrong_epoch_dir, mut wrong_epoch) =
            matched_case(&root, "wrong-epoch", &binding, &founder, &joiner);
        wrong_epoch.expected_epoch = [0xee; 32];
        let error =
            admit_invitation(&wrong_epoch_dir, &wrong_epoch, &joiner, &settings()).unwrap_err();
        assert!(error.contains("does not name"), "{error}");

        // Membership heads the inviter pinned but Gemot did not converge to.
        // This is what stops a welcome minted before a removal from handing the
        // joiner a key a departed member still holds.
        let (stale_heads_dir, mut stale_heads) =
            matched_case(&root, "stale-heads", &binding, &founder, &joiner);
        stale_heads.membership_heads = vec![[0xaa; 32]];
        let error =
            admit_invitation(&stale_heads_dir, &stale_heads, &joiner, &settings()).unwrap_err();
        assert!(error.contains("did not converge"), "{error}");

        // The inviter's own time bound, on an otherwise valid envelope, with a
        // clock moved past it. Nothing is created, so expiry is cheap and
        // cannot be reached by an envelope that would have failed anyway.
        let expired_dir = root.join("expired");
        let expired_clock = PlaceWorkerSettings {
            authority_clock: AuthorityClock::Fixed(invite.not_after_ms + 1),
            ..PlaceWorkerSettings::default()
        };
        let error = admit_invitation(&expired_dir, &invite, &joiner, &expired_clock).unwrap_err();
        assert!(error.contains("expired"), "{error}");
        assert_eq!(residue(&expired_dir), (false, false, false));

        // Aliased Commons scopes are refused before any store is created.
        let aliased_dir = root.join("aliased");
        let mut aliased = invite.clone();
        aliased.binding.chat = ChatSpaceId(aliased.binding.root.0);
        let error = admit_invitation(&aliased_dir, &aliased, &joiner, &settings()).unwrap_err();
        assert!(error.contains("same scope"), "{error}");
        assert_eq!(residue(&aliased_dir), (false, false, false));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_authored_invitation_admits_on_the_other_side() {
        let root =
            std::env::temp_dir().join(format!("turnstone-place-author-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let founder = InMemoryProvider::from_seed([0xc1; 32]);
        let joiner = RootIdentity::Unsealed(InMemoryProvider::from_seed([0xc2; 32]));
        let outsider = RootIdentity::Unsealed(InMemoryProvider::from_seed([0xc3; 32]));
        let binding = binding(0x71);
        let host = root.join("host");
        let guest = root.join("guest");

        // Both sides prepare a durable group identity before any envelope
        // exists. This is the precondition, not a step in the flow.
        let joiner_prekey = prepare_group_identity(&guest, &joiner, binding.moot.0).unwrap();
        found_place_for_authoring(
            &host,
            &binding,
            &founder,
            joiner.master_public_key().to_bytes(),
        );
        // The host founds its crypto group; the guest only prepares an
        // identity. Different preconditions for different roles.
        found_place_group(&host, &founder, binding.moot.0).unwrap();
        assert!(
            found_place_group(&host, &founder, binding.moot.0).is_ok(),
            "founding twice must not strand the epochs already handed out"
        );

        // Someone the Moot never admitted cannot be invited, and the refusal
        // names the real reason rather than minting an envelope that could
        // only ever be refused on arrival.
        let outsider_prekey =
            prepare_group_identity(&root.join("outsider"), &outsider, binding.moot.0).unwrap();
        let error = author_invitation(
            &host,
            &binding,
            &founder,
            &outsider_prekey,
            AUTHORITY_AT_MS + 1_000,
            &settings(),
        )
        .unwrap_err();
        assert!(error.contains("does not contain the invited root"), "{error}");

        let invite = author_invitation(
            &host,
            &binding,
            &founder,
            &joiner_prekey,
            AUTHORITY_AT_MS + 1_000,
            &settings(),
        )
        .unwrap();

        // The product path produced it and the product path accepts it: no
        // test fixture stands between the two sides.
        let admitted = admit_invitation(&guest, &invite, &joiner, &settings())
            .expect("an authored invitation admits");
        assert_eq!(admitted.binding, binding);
        assert_eq!(admitted.moot.members, 2);
        assert_eq!(admitted.group_members, 2);
        assert_eq!(residue(&guest), (true, true, true));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Found a Moot with both roots in its membership fold, as the authoring
    /// side would already have.
    fn found_place_for_authoring(
        directory: &Path,
        binding: &PlaceBindingV1,
        founder: &InMemoryProvider,
        joiner_root: [u8; 32],
    ) {
        let founder_id = founder.master_public_key().to_bytes();
        let stores = place_store_dir(directory);
        std::fs::create_dir_all(&stores).unwrap();
        let moot = pollster::block_on(MootFile::open(
            stores.join("gemot"),
            MootId(binding.moot.0),
            founder_id,
            settings().retention,
        ))
        .unwrap();
        pollster::block_on(moot.found(
            founder.master_keypair().to_seed(),
            None,
            None,
            place_rules(founder_id),
            1,
        ))
        .unwrap();
        pollster::block_on(moot.membership_store().author_for_identity(
            founder,
            MootMembershipAction::Create {
                initial_members: vec![MootMember {
                    member: founder_id,
                    access: MootAccessLevel::Manage,
                }],
            },
        ))
        .unwrap();
        pollster::block_on(moot.membership_store().author_for_identity(
            founder,
            MootMembershipAction::Add {
                member: joiner_root,
                access: MootAccessLevel::Write,
            },
        ))
        .unwrap();
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
