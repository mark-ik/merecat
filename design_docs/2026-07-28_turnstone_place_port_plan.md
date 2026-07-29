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
under the next generation. The fresh Turnstone library binary passes 154
tests, with 4 ignored external-endpoint receipts.

### T3. Live graph and chat

- Join the Gemot, Commons graph, and Commons chat lanes.
- Add authority preflight, `ShareFocusedNode`, `SendPlaceMessage`, and
  `ResyncPlace`.
- Add `src/place/projection.rs` and reconcile into Canvas.
- Persist key changes only through the sealed secret store.

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
proof in the peer-web reframe. Only then may `peer` become a default feature.

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
- Do not show `Replica::projection()` as shared truth; use the
  authority-filtered materialization.
- Do not overwrite the private local overlay when shared state converges.
- Do not route Knot edits through Commons graph batches.
- Do not call a chat channel Murm or merge their grammars.
- Do not claim shared-graph confidentiality before that lane is encrypted.
- Do not begin calls, co-browsing presence, Outrider service publication, or a
  second carrier before the headed place receipt.
