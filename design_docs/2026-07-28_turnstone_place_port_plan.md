# Turnstone place port

**Date:** 2026-07-28

**Status:** active implementation plan; follows the Commons promotion and
stops at the first headed two-peer receipt.

This is the file-level expansion of P2 and P3 in the
[peer-web reframe](./2026-07-28_turnstone_peer_web_reframe.md). It is grounded
in the live Gemot, Commons, Stickleback, transport, Knot, and Turnstone APIs.

## Decision

A place is a Turnstone product composition over reusable Mere-side domains. It
is not another application or authority layer.

- Gemot decides the Moot's governance and converged authority.
- Commons owns the shared graph and channel grammars.
- Knot owns shared document history.
- Murm owns direct and small-group conversation outside a place channel.
- Stickleback owns retained-operation processing, reconciliation, and group
  encryption machinery.
- Turnstone owns session binding, product commands, projection into its
  surfaces, local overlays, and user-visible status.

`Action`, `Effect`, `Update`, `place.json`, and every observation type remain
carrier-neutral. The first worker uses p2panda because that is the proven
carrier. A second carrier must prove a shared seam before the worker is
abstracted into another crate.

## Corrections from the live APIs

The live audit found four boundaries. The first three are now resolved in
Mere:

1. Gemot membership has a signed wire/store/drop lane, and `Moot` exposes its
   constitution and membership stores.
2. Stickleback has a production `GroupSession` with versioned pre-key,
   broadcast-control, and addressed-direct frames. Its pre-key binds the
   self-authenticating DCGKA recipient to a Personae root.
3. Commons graph and chat replicas can attach and verify a Personae
   derived-writer attestation. Their projections expose the stable root, and
   chat mutation and checkpoint authority use that root rather than the
   space-derived signing key.
4. Commons graph operations remain signed plaintext CBOR. Commons chat uses
   the durable-data keyring. The first place proof can establish graph
   integrity and authorization, but it must not claim shared-graph
   confidentiality.

These are substrate gates, not reasons to move authority into Turnstone.

## Session truth

A personal session keeps today's meaning: `graph.json` is its graph truth.

A shared session has four distinct durable parts:

| Path | Meaning |
| --- | --- |
| `place.json` | public, versioned binding to one Moot and its shared spaces |
| `graph.json` + `facets.json` | offline projection cache plus explicit private local overlay |
| `place/*.redb` | retained Gemot and Commons operations |
| `place-secrets/*` | Personae-sealed group-session and keyring state |

For a shared session, an effective Commons projection is the authority for
shared graph facts. `graph.json` is never a competing authority. It permits
offline startup and also holds local browsing nodes that the person has not
published.

`ShareFocusedNode` promotes a local node by authoring a Commons `Container`
with the same stable Turnstone UUID. Once that operation is effective, the
projection marks the node shared. Revocation removes the shared projection. A
node survives as a private local node only if it had an explicit local origin
before it was shared.

Shared edges receive a deterministic semantic statement id derived from the
Commons `EdgeId`. That lets a later projection replace shared relations
without deleting local relations between the same nodes. Layout, cameras,
pane state, browser state, and other presentation facets remain local.

## Stable data vocabulary

`src/place.rs` owns app-side values only. It imports no transport or domain
service type.

```rust
pub struct PlaceId(pub [u8; 32]);
pub struct SharedContainerId(pub [u8; 32]);
pub struct ChatSpaceId(pub [u8; 32]);

pub struct PlaceBindingV1 {
    pub version: u16,
    pub moot: PlaceId,
    pub root: SharedContainerId,
    pub chat: ChatSpaceId,
    pub default_channel: String,
}
```

The ids stay separate. A Moot id is not a Commons container id; a chat space
id is not the string id of a channel inside that space. Knot documents are
addressed nodes in the shared graph and do not add another root field.

The invitation is a host envelope, not a certificate:

```rust
pub struct PlaceInviteV1 {
    pub version: u16,
    pub binding: PlaceBindingV1,
    pub governance: ArtifactRefV1,
    pub key_welcome: ArtifactRefV1,
    pub rendezvous: Vec<RendezvousV1>,
}

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

pub struct RendezvousV1 {
    pub carrier: String,
    pub hint: String,
}
```

The first recognized rendezvous tag is
`p2panda.endpoint-ticket.v1`. Unknown tags survive parsing but are not dialed.
The first governance artifact is an aggregate Gemot native drop. The first key
artifact must be the eventual recipient-bound Stickleback welcome, not a raw
serialized `DataKeyring`.

Every artifact is bounded before allocation or fetch and checked against its
declared digest. Import succeeds only when:

1. the Gemot evidence addresses `binding.moot`;
2. the converged membership fold contains the local Personae root;
3. the sealed group session is bound to that local Personae root, and the
   welcome is addressed to its authenticated crypto recipient and produces the
   named group epoch;
4. Gemot binds that epoch to the same membership heads;
5. the Commons root and chat space are valid governed scopes.

Only then does Turnstone persist `place.json`. Possessing or forwarding the
envelope alone grants nothing.

## App state and vocabulary

`App` holds a data-only `PlaceState`:

```text
PlaceState
  binding
  phase: closed | opening | ready | offline | degraded
  moot: declaration, roster, checkpoint, authority revision
  graph: projection digest, shared ids, pending causality, pending/revoked authority
  chat: channels, current messages, pending causality
  sync: status per retained lane
  last failure
```

The first actions are:

- `JoinPlace(PlaceInviteV1)`
- `SendPlaceMessage { channel, body }`
- `ShareFocusedNode`
- `ResyncPlace`

Existing `OpenAddress`, `OpenInWorkbench`, content spawning, and Knot editor
save actions remain the way a person opens and edits a shared Knot document.
The place port supplies discoverable Knot addresses and the shared endpoint
configuration; it does not invent duplicate `OpenSharedKnot` or
`SaveSharedKnot` actions.

Effects carry an app-generated request id and the current session id:

- `OpenPlace { session, binding }`
- `JoinPlace { session, request, invite }`
- `RunPlaceCommand { session, request, command }`
- `ClosePlace { session }`

Every place update returns both the session id and a worker generation. An
update from a departed session is dropped explicitly. The update vocabulary
contains app-owned snapshots and receipts, never `Operation<E>`, transport
tickets, store handles, or key material.

Local authoring follows two authority paths:

- Turnstone preflights its own command against the current Gemot authority and
  refuses an unauthorized command before authoring.
- Received Commons operations pass structural admission and remain retained,
  then the effective projection excludes pending or revoked authority. This
  preserves later reevaluation without showing unauthorized content as truth.

## Shell-owned worker

`src/place/worker.rs` runs a dedicated async worker. It owns:

- one derived transport identity and `P2pandaTransport`;
- the Gemot aggregate and its joined retained lanes;
- one Commons graph replica and joined lane;
- one Commons chat replica, keyring, and joined lane;
- group membership/control state;
- redb backends and sealed secret state.

The app and pane runners own none of these handles.

For the first carrier, one imported endpoint ticket is registered and tagged
for the Moot, Commons-root, and chat-space LogSync overlays. The worker uses
`JoinedSpace`; it does not hand-build p2panda `LogSync` sessions or duplicate
domain folds.

Ticket-based joining is also the only lane that works everywhere today.
macOS Local Network policy blocks multicast discovery for unsigned resident
binaries, so mDNS-style automatic discovery cannot carry the first proof and
must not become an implicit dependency of T3a. Automatic discovery needs its
own signed-app receipt before it is claimed on any platform.

Opening is staged and reported:

1. validate the binding and invitation bounds;
2. open the sealed group state and domain stores;
3. import and verify membership, governance, and recipient welcome;
4. register rendezvous and join every required lane;
5. materialize Gemot, authority-filtered Commons, and chat snapshots;
6. emit `ready` only when the binding, authority revision, and key epoch agree.

A failed optional rendezvous yields `offline` when local materialization is
usable. Missing authority or key state yields `degraded` and disables the
affected authoring commands. Partial joins never masquerade as ready.

On a session switch the worker drops every joined lane before opening the next
session. On `TrashSession`, the shell waits for a worker release
acknowledgement before moving the directory, just as it already does for the
recycle-bin store on Windows.

## Projection bridge

`src/place/projection.rs` converts the effective
`GraphLog<Container, Relation>` into Turnstone graph mutations.

- A UUID-shaped `Container.id` is preserved.
- Any other id maps deterministically from `(Commons root, container id)` and
  retains the source id in a local provenance facet.
- The primary address becomes the Turnstone node address. An addressless
  container receives an internal `turnstone://place/...` address.
- Title, tags, media type, inline body, content reference, and nested Knot log
  id are preserved where the Turnstone graph has a corresponding field.
- Relation class and label lower to a semantic assertion with the stable
  Commons statement id.

The reconcile pass changes only previously marked shared nodes and statements.
It leaves the private overlay and every local presentation facet intact.
Unit tests cover id stability, addressless nodes, parallel relations, a local
node becoming shared, and revocation with and without a prior local origin.

## Required Mere substrate slice

Finish the current Commons promotion, then land one bounded bootstrap slice
before implementing invitation import in Turnstone:

1. **Commons writer identity (done 2026-07-29):** `Replica::edit` carries a verified
   `DerivedKeyAttestation`; prove a writer derived from Personae is effective
   under a Gemot grant to its root, then becomes revoked without deleting the
   retained operation.
2. **Gemot live aggregate (done 2026-07-29):** expose the constitution lane
   through `Moot`, and give the membership fold a signed durable
   wire/store/drop path. Prove a late peer reconstructs constitution,
   delegation, membership, object, and Tessera state through the aggregate.
3. **Stickleback group session (done 2026-07-29):** promote DCGKA
   create/welcome/update processing from the test into a serializable
   production boundary. Prove only the addressed member recovers the epoch,
   removal rotates it away, and restart restores the same state from a
   Personae-sealed record.
4. **Chat identity binding (done 2026-07-29):** a versioned encrypted chat
   record binds its derived signing key to its stable Personae root. Admission,
   projection, message mutation, and checkpoint authority all verify or use
   that root rather than accepting a host-side assertion.

This slice belongs in existing Mere packages. It does not found a new product
or a second application shell.

Gemot receipt: membership operations carry a verified Personae root
attestation, survive Redb reopen, and materialize deterministically through
p2panda-auth. `Moot` exposes constitution and membership sync stores plus a
membership command, snapshot, and outbound lane. A fresh aggregate imports one
or more signed operations in all five retained domains and matches the source
snapshot. `cargo test -p gemot --offline` passes 107 tests; strict library
Clippy passes.

Stickleback receipt: `GroupSession` owns serializable DCGKA, data epochs, and
per-author control sequence state. Versioned pre-key, broadcast-control, and
addressed-direct frames are production exports. A group-scoped
Personae-derived key signs each crypto recipient and pre-key; a tampered
binding fails verification, and control processing checks the authenticated
operation root against that binding. The executable receipt covers initial
creation, addressed epoch recovery, update, removal rotation, a late bundle
welcome at a nonzero control sequence, and restart through
`SealedRecordStorage`.
`cargo test -p stickleback` passes 55 unit and 5 boundary tests; strict library
Clippy passes.

Commons chat receipt: `ChatReplica::for_identity` derives a space-scoped writer
and carries its Personae attestation inside each encrypted signed event or
checkpoint. Admission rejects a foreign-root claim and a cross-space
attestation replay before storage. Projection exposes the stable root, and edit,
delete, and checkpoint authority compare that root. A real Gemot delegation
classifies the projected author effective and then revoked while both retained
operations remain stored. `cargo test -p commons-spine` passes 40 tests; strict
library Clippy passes. This receipt proves the identity seam. Authority-filtered
chat reprojection remains worker composition work.

Shared-graph encryption is a later explicit profile change unless it is pulled
into the first product proof. Until then, receipts say “signed and
authority-filtered shared graph” and say nothing about graph confidentiality.

## Implementation order

### T0. Contract and persistence (implemented 2026-07-29)

- Add `src/place.rs` with the data vocabulary and validation.
- Add `place.json` load/save helpers and round-trip/unknown-version tests in
  `src/session.rs`.
- Add `PlaceState` to `App`; personal sessions remain unchanged when the
  sidecar is absent.

Done when a personal session round-trips byte-for-byte as before, a valid
binding survives a switch and restart, and a malformed binding produces a
visible place failure without losing `graph.json`.

Receipt: `cargo test --lib place_binding_ -- --nocapture` passes the public
sidecar, unknown-version, switch/restart, and malformed-binding cases. The
T0 binding initially opened as an explicitly stale offline cache. T2 now
supersedes that placeholder with worker-owned retained-state opening.

### T1. Mere bootstrap gate (implemented 2026-07-29)

Land the four required substrate items above. Stop Turnstone network work if
any proof uses a raw shared key, a test-only membership implementation, or a
host-supplied root identity assertion.

Receipt: all four substrate items now have production seams and executable
receipts in Mere. Turnstone place work may proceed to T2 without supplying its
own membership, group-key, or author-root assertions.

### T2. Worker and offline opening (implemented 2026-07-29)

- Add `src/place/worker.rs` and the shell handle/update receiver.
- Open stores, sealed state, and an existing binding.
- Materialize cached/offline Gemot, Commons, and chat state.
- Wire release/reopen into switch, trash, and shutdown ordering.

Done when two local profiles reopen their own retained place state and stale
updates from the prior session cannot change the active app.

Receipt: the shell-owned place worker opens the existing Gemot constitution,
Commons graph and chat Redb stores, and the Personae-sealed Stickleback group
session. Gemot discovers its founder only from a verified retained signed
genesis; Turnstone does not duplicate that root assertion in `place.json`.
The sealed group session restores the data-key epochs needed to read retained
chat.

`two_profiles_reopen_their_own_retained_place_state` reopens distinct graph,
chat, governance, and group snapshots for two local profiles.
`stale_place_update_from_a_departed_session_is_ignored` proves that both the
session id and opening generation guard app adoption.
`worker_releases_files_before_reopen_and_advances_generation` waits for a
worker release acknowledgement, moves the session directory, and reopens it
under the next generation. The Turnstone library passed 154 tests with 4
ignored external-endpoint receipts at T2, and 155 with the same 4 ignored as
of 2026-07-31 after T3.0.

One correction from T3.0, recorded because it invalidated a reading of this
rung: the T2 fixture founded its Moot and then granted the profile nothing, so
every fact it authored classified as pending the moment real authority was
applied. The `(1, 1)` and `(2, 2)` assertions above held only because the
worker was projecting unfiltered. The fixture now issues a real founder
delegation. A cached-state test that never admits anyone cannot tell an
authority regression from a working filter.

### T3. Live graph and chat

Split into three rungs on 2026-07-31. The original single rung said "join"
without giving invitation validation its own gate, even though `PlaceInviteV1`
is specified above and only `PlaceBindingV1` exists in code. Admission is where
a place either is or is not governed, so it earns a rung.

#### T3.0 Authority-correct projection - DONE 2026-07-31

Prerequisite to everything below, because it decides what a surface is even
allowed to show.

`ChatReplica` gained `projection_with_authority`; it previously had no
authority-filtered projection at all, only a classifier proven in a test. The
place worker now folds Gemot's own delegations and constitution rules into one
`GemotAuthorityView` and projects both Commons domains through it. `ChatCache`
reports pending and revoked counts alongside the graph's.

Receipt: `a_revoked_member_reaches_no_projected_place_state`. An admitted
member projects 2 nodes and 2 messages; after a signed revocation on the
retained Gemot lane, 0 nodes, 0 edges, 0 messages, 0 channels, with the
withheld operations counted as revoked and the certificate still retained.
Withheld, not erased.

Authority evaluation time is a host input (`AuthorityClock`), because
delegation windows are absolute and the pending/revoked distinction depends on
it. Tests pin it. A clock error reads as 0, under which nothing has opened yet,
so unreadable time withholds content rather than admitting it.

#### T3a. Invitation and admitted join - envelope landed 2026-07-31, admission open

- ~~Parse and bound `PlaceInviteV1`~~ done: `src/place/invite.rs`. Digests
  checked against bytes, inline artifacts and rendezvous lists bounded before
  they are trusted. An `Addressed` artifact returns `UnfetchedArtifact` rather
  than an empty slice, so an admission step cannot read "not fetched" as
  "verified empty". An unknown carrier survives parsing and never appears in
  `dialable()`, so adding a carrier is not a breaking change.
- ~~Verify the Gemot governance artifact and the recipient-bound Stickleback
  welcome~~ done: `admit_invitation`. The drop is imported into the place's own
  Gemot store, so the fold is Gemot's rather than a claim the envelope makes;
  membership must contain the local root; the welcome's control and direct
  frames must name this group, pair with each other, and address this
  recipient's registered crypto identity; and the DCGKA transition must produce
  a current epoch.
- ~~Persist only after admission succeeds~~ done for the sealed group session,
  which is written on the last line of the admitted path and nowhere else. A
  refusal removes the store and secrets directories, so a partial import cannot
  be reopened later as a retained place.
- ~~Persist `place.json` only after admission~~ done, and made structural
  rather than a rule. `save_place_binding` now has one caller in the product
  path: admission's last line, after the group session is sealed, so the file's
  existence implies the whole check list ran. Routine session saves use
  `update_place_binding`, which writes only when a binding is already present
  and otherwise reports that it wrote nothing. A save can update an admitted
  binding; it can never mint one. App state holding a binding with no admitted
  sidecar is logged as the anomaly it is rather than silently repaired.
- ~~Invitation expiry~~ done. `not_after_ms`, checked against the host
  `AuthorityClock` before anything is created. Kept separate from
  `membership_heads` on purpose: the heads pin is a security bound the domain
  enforces and cannot be relaxed, while expiry is a time bound the inviter
  chooses, so a forwarded envelope stops working even in a Moot whose roster
  never moves. T3a's done-when named "expired" and nothing could express it.
- ~~Ticket encoding~~ settled, with no new format to define.
  `p2panda.endpoint-ticket.v1` means the string from
  `P2pandaTransport::ticket()`, which is `EndpointTicket::to_string()`, and it
  is consumed by `add_peer_ticket`, which conveniently returns the `PeerID`
  that `set_topics` needs to bootstrap the overlays.

  Checked against upstream rather than assumed: `iroh` 1.0.3, `iroh-base`
  1.0.3, and `iroh-tickets` 1.0.0 all resolve to crates.io with checksums.
  `p2panda-net` is a fork (`mark-ik/p2panda`) but does not fork iroh or the
  ticket format, so the encoding carries no fork-specific risk. The one forked
  iroh-family crate in the graph is `iroh-mdns-address-lookup`, which is
  discovery, the lane already excluded from the first proof.
- ~~Join the lanes~~ **DONE 2026-07-31.** The receipt is
  `a_joiner_catches_up_on_a_live_place_over_one_ticket`: a founder's retained
  place (Moot with two members, a founder self-delegation, two graph nodes,
  one channel, two sealed messages), one endpoint ticket in the invitation,
  and the worker's own `Join` command dialing it. Seven lanes per side over
  one endpoint each, one-directional bootstrap (the host never learns the
  joiner ahead of time), and `Resync` folding the converged state through the
  same authority filter as every offline open. Render-free, product path on
  the joiner side end to end.

  The pieces landed where the scoping said they belonged: `Moot::join_lanes`
  and `Replica::join` in mere, `LiveLanes`/`join_live` in
  `src/place/lanes.rs` here, with the worker still synchronous and the tokio
  runtime owned by the lane handles. A ticketless invitation still admits and
  stays offline. The worker gained `Resync`, which re-folds projections
  without touching lanes; the `ResyncPlace` app action wires to it in T3b.

  One fixture-level fact worth keeping: a root capability grant alone makes
  nothing effective, because `MootDelegations::covers` walks certificates
  only. A founder whose content should project must delegate to itself.

  The scoping sections below are kept as written; the lane-count correction,
  the ALPN defect, and the publish requirement all came from them.

#### T3a lane join, scoped 2026-07-31

**It is not three lanes.** This plan has said "the Gemot, Commons graph, and
Commons chat lanes" throughout, which reads as three `JoinedSpace`s. Gemot is
five stores, each with its own extension type, log-id type, and accept closure:
constitution, delegation, membership, records/objects, tessera. With the two
Commons lanes that is seven.

T3c's receipt needs five of them. Constitution, delegation, and membership
carry authority correctness; graph and chat carry the content. Records and
tessera feed roster and trust display, not the proof, and can follow.

**All five Gemot lanes subscribe to the same topic**, the Moot id, and are
distinguished only by extension type. Gemot's existing lane proofs each join
one lane in isolation, so five lanes sharing one topic on one endpoint is
untested in combination: a session will receive every message on the topic and
reject what does not decode as its own extension. Verify that with two lanes
before building five. If it does not hold, the fix is domain-side topic
separation, not a Turnstone workaround.

**Resolved 2026-07-31: the check found a real defect, and it is fixed.** Two
lanes on one endpoint could not coexist at all: every LogSync session
registered the same hardcoded protocol id (ALPN), the endpoint keeps one
handler per id, and the last-joined lane silently received all inbound sync.
Topic separation was tried and is NOT the fix; routing happens at the ALPN
before any topic is read, and the store's topic registration pins the sync
topic to the Moot id anyway. The fix is fork-side
(`LogSync::builder().protocol_id(...)`, p2panda `c880761`) surfaced as a
required `lane` argument on `JoinedSpace::join` (mere `6300d0da`), with lane
ids scoped to kind plus space. Both join orders now converge in ~6s.
**Push order matters: the fork commit must reach mark-ik/p2panda before mere
is pushed**, since mere's manifest tracks that branch and its stickleback now
calls the new builder method.

The same investigation confirmed the publish requirement from below: freshly
authored operations reach live peers through `JoinedSpace::publish`, not
through implicit re-sync. Initial sync covers retained state for a late
joiner; the product paths must push what they author.

**Most of the ceremony belongs in mere, not here.** The stop rule already says
Turnstone does not assemble p2panda sessions, and the domains mostly agree:

| Lane | State |
|---|---|
| Commons chat | `ChatReplica::join(endpoint, gossip)` exists, with policy, key state, and checkpoint authority already inside its accept closure |
| Commons graph | Only `sync_store()`. The ceremony is written inline in tests; it wants a `Replica::join` beside chat's |
| Gemot's five | Sync proofs exist per lane; no product helper. Wants one call that joins the set and hands back the handles |

So the first move is mere-side: `Replica::join`, and a Gemot lane-set join.
Turnstone then holds handles and composes, which is what it is supposed to do.

**What Turnstone owns.** The worker gains a tokio runtime, scoped to transport
futures rather than the mass `pollster` conversion originally imagined here:
the existing offline calls await redb, in-memory stores, and Stickleback
crypto, none of which need a reactor. Live handles sit beside `OpenPlace` and
drop on `Release`, under the same acknowledgement discipline that already lets
a session directory move. Ticket to peer is
`add_peer_ticket(hint)` → `PeerID` → `set_topics(peer, overlays)`, where the
overlays are `sync_overlay_topic` of the Moot, Commons root, and chat space.

**Two things the offline rungs left implicit.**

Re-projection happens only at `Open` today. Live lanes need received operations
to trigger a re-fold and a new snapshot, so an accept closure firing must reach
the app as an update. Debounce it: one update per operation would be a redraw
per received message.

Ongoing DCGKA processing does not exist. The joiner processes exactly one
frame, ever. Adds, removes, and rotations after join have nowhere to go, and
the membership lane is what will deliver them. `save_group_session` already
gives them a persistence path.

**Stop rule, before anyone optimizes.** The worker must keep folding chat from
full history. `projection_from_checkpoint_with_authority` is cheaper and
filters only the retained tail, so adopting it would silently grandfather
revoked content that a checkpoint already committed.

**Four spec corrections found by implementing it.** `PlaceInviteV1` as drafted
could not be admitted at all.

1. `inviter`. Stickleback needs an authenticated author root to process a
   control frame and an envelope cannot authenticate its own.
2. `inviter_prekey`. A welcome cannot be processed without the sender's
   authenticated pre-key, and a freshly prepared identity knows only its own.
   The bundle carries a Personae attestation, so it also turns `inviter` from a
   claim into a verified fact: admission requires the attested root and the
   declared one to agree, *and* the root to be in the Gemot membership fold.
3. `founder`, distinct from `inviter`. Conflating them was a real bug: opening
   the Gemot store with the inviter as founder refuses every invitation from an
   ordinary member, which is the normal case. It is not a trust decision, since
   genesis admission requires the retained genesis to be authored by and to
   name exactly that root. It is carried only because importing a drop needs a
   founder before the fold exists, and founder discovery needs the fold.
4. `key_welcome` split into `key_welcome` plus `key_direct`. Stickleback
   publishes bounded, version-checking decoders for `GroupControlFrame` and
   `GroupDirectFrame` and none for `GroupSessionDispatch`, so carrying the
   frames separately means a peer-supplied welcome is parsed by its own domain
   rather than by a raw CBOR decode into a struct with private fields.

**Being invitable is a precondition, not a consequence.**
`GroupSession::new` draws its long-term key from the RNG, so a recipient id is
not derivable from a Personae root. The session that generated the published
pre-key is the only one a welcome can ever be addressed to, and creating a
fresh session at admission time refuses every genuine welcome. Hence
`prepare_group_identity`, which is idempotent so a second call cannot rotate
the identity out from under a welcome already in flight.

This also corrects the refusal path. Deleting `place-secrets` on a refused
invitation would let any stranger destroy the key material a pending welcome is
already addressed to, so refusal now removes only a Gemot store this attempt
created and never touches sealed secrets.

**Check 4 now holds.** The earlier worry about key types was misplaced: a
`GroupSecretId` is the SHA-256 *of* a secret, not the secret, so `expected_epoch`
names an epoch without disclosing anything and the no-key-types discipline is
intact.

The substantive half is `membership_heads`. Pinning the epoch alone would not
be enough: an epoch minted before a removal still decrypts, so a welcome could
hand a joiner a key a since-departed member also holds while every other check
passed. Admission requires the pinned heads to equal the heads Gemot itself
converged to from the imported evidence.

**Delivery is not evidence.** An invitation may reasonably arrive over a Murm
thread with someone already trusted, over a pasted link, or over a carrier.
Admission is identical in every case and a trusted sender shortcuts nothing.
This is structural rather than a rule: `admit_invitation` takes no channel,
peer, or session argument, so there is nowhere for delivery trust to enter.
Keep it that way when Murm becomes a real delivery path, and when a Murm
rendezvous carrier is added it goes in the `rendezvous` list, which already
tolerates unknown tags without refusing the envelope.

Done when a malformed, expired, or foreign-recipient invitation leaves no
`place.json` and no sealed secret behind, and a valid one joins all three
lanes.

#### T3b. Effective graph and chat

- Add `ShareFocusedNode`, `SendPlaceMessage`, and `ResyncPlace` with authority
  preflight.
- Add `src/place/projection.rs` and reconcile into Canvas without overwriting
  the private local overlay.
- Persist key changes only through the sealed secret store.

Done when authored facts converge between two peers and the T3.0 filter still
holds on live lanes, not only on cached opens.

#### T3c. Render-free two-peer receipt

Done when the render-free two-peer test covers join, message, shared HTTPS
node, partition, restart catch-up, and an unauthorized graph write that never
enters the effective projection.

### T4. Knot through the existing content lane

- Publish a Knot document address/container through Commons.
- Configure the existing `KnotAuthoringEngine` for the active place endpoint.
- Keep Knot revisions and merge receipts in Knot; Commons carries the document
  address and metadata, not translated document edits.

Done when both peers author offline revisions and reopen the same derived
document after convergence.

### T5. Headed receipt

- Bind Roster, Comms, Canvas, Workbench, and Steward to `PlaceState`.
- Extend observation with Personae roots, place ids, lane status, operation
  receipts, authority outcomes, and projection digests.
- Drive two actual Turnstone processes and capture both presented windows.

Done when the machine-readable receipt and both captures satisfy the seven-step
proof in the peer-web reframe.

This rung previously ended "only then may `peer` become a default feature".
There is no `peer` feature and there will not be one: see the P1 amendment in
the reframe. What T5 gates instead is showing shared place content on a
product surface at all, and it is the gate the Commons calls plan's A1 waits
on.

## File seams

| File | Change |
| --- | --- |
| `src/place.rs` | app vocabulary, validation, worker handle |
| `src/place/worker.rs` | async service ownership and first p2panda backend |
| `src/place/projection.rs` | Commons-to-Turnstone projection and local-overlay reconcile |
| `src/action.rs` | four actions, correlated effects, typed updates |
| `src/app/mod.rs` | `PlaceState`, no service handles |
| `src/app/session_lifecycle.rs` | load binding, mark cache stale, emit open/close effect |
| `src/app/updates.rs` | generation check, snapshot/rejection fold |
| `src/session.rs` | `place.json` only; never key bytes |
| `src/shell/mod.rs` | place worker handle and update receiver |
| `src/shell/effects.rs` | command lowering and release/reopen ordering |
| `src/shell/events.rs` | drain place updates through `App::apply_update` |
| `src/knot_authoring.rs` | active-place endpoint configuration, existing editor unchanged |
| `src/observe.rs` | place, authority, sync, and digest receipts |
| `scenarios/` | render-free orchestration followed by the headed two-process proof |

## Stop rules

- Do not persist a raw group key, DCGKA state, or welcome in `place.json`.
- Do not treat an endpoint ticket, relay identity, invite envelope, or local
  session as content authority.
- Do not show `Replica::projection()` or `ChatReplica::projection()` as shared
  truth; both route through `AllowAllAuthority` and exist for local authoring
  and admission, not for display. Use `projection_with_authority` on each.
- Do not overwrite the private local overlay when shared state converges.
- Do not route Knot edits through Commons graph batches.
- Do not call a chat channel Murm or merge their grammars.
- Do not claim shared-graph confidentiality before that lane is encrypted.
- Do not begin calls, co-browsing presence, Outrider service publication, or a
  second carrier before the headed place receipt.
