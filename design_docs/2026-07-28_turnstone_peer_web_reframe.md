# Turnstone Peer Web Reframe

**Date:** 2026-07-28
**Status:** active product direction and bounded implementation plan.

## Decision

Turnstone is a local-first browser for shared, addressable places where people
talk, author, publish, and bring other webs into view.

It does not need a second product name for the combination of Gemot, the
Commons profile, and Knot. `commons` remains a technical profile name. A moot
is one durable governed place. Turnstone is the application through which a
person inhabits personal and shared places.

This changes the product emphasis:

- the primary address space contains people, murmurs, moots, channels,
  documents, services, and remote resources;
- the graph is durable shared state, not merely browser history or a visual
  index;
- chat is the smallest shared place, and Knot supplies authored documents;
- co-browsing is a live relationship over the same addressed objects;
- HTTP and the smolweb protocols are languages and resource lanes within the
  peer web;
- Scry, Graft, and Weld attach incumbent web surfaces without defining the
  product.

Protocol agnosticism applies to addressing, identity, discovery, carriage, and
composition. Protocol semantics remain intact. Gemini, Gopher, Misfin, LXMF,
HTTP, Murm posts, and Commons messages do not become one generic wire object.

## Product model

| Owner | Product question |
| --- | --- |
| Personae | Who is acting, and what identity proofs root that actor? |
| Kith | What portable trust and capability may that person carry? |
| Murm | How do known peers converse and catch up? |
| Gemot / Moot | What durable place is this, who belongs, and what rules apply? |
| Commons profile | How do shared graph and channel facts converge? |
| Knot | What document is being authored, conflicted, or resolved? |
| Stickleback | How are accepted operations retained, reconciled, and carried? |
| Turnstone | How does a person browse, talk, author, and participate? |
| Genet engine lanes | How is a local or remote resource presented and operated? |
| Graphshell | How is selected state projected to another client? |

Murm and Gemot remain domain packages in `mere.git`. The default `mere` facade
stays an offline graph library. Turnstone may depend directly on the peer
domain packages, as it already does for Personae, Chartulary, Codicil, Knot
support, and other packages housed in `mere.git`. Repository ownership and
facade ownership are separate decisions.

Kith is the settled name for the capability-sharing layer, not yet a package
in the live tree. Current proofs are owned by Personae delegation, Servitor
typed scopes, and Gemot's authority folds. Founding Kith later must move a real
shared responsibility rather than duplicate those proofs.

## Live boundary map

The substrate is further along than the product:

- Turnstone already uses the user's Personae root and has a real Knot
  authoring surface with typed effects and headed receipts.
- Turnstone reserves a Comms pane, but `App` contains no conversation, place,
  membership, channel, presence, or sync state. The pane has no live port.
- `mere/crates/shell/comms` has a host-neutral inbox model and a working Murm
  adapter. Its Graphshell/Meerkat descriptions are stale, and it has no
  Turnstone consumer.
- Gemot exposes a high-level single-Moot aggregate with commands, snapshots,
  authorization, durable storage, drops, and outbound operations.
- the Commons graph and chat implementations have Memory, Redb, LogSync,
  authority, encryption, native-drop, Reticulum, and direct-PHY receipts, but
  `commons-spine` still lives under `crates/probes` outside Mere's workspace.
- Knot already consumes the neutral causal machinery from Stickleback. It
  remains the document authority and must not be translated through the graph
  merely to share a replication lane.
- Turnstone sessions are durable local graph workspaces. Nothing yet binds a
  session to a Moot and its shared root container.

The missing work is product composition and one real authority adapter. It is
not another replication design.

## Target host shape

Turnstone keeps its existing action/effect/update discipline:

```text
human, script, peer, or scenario intent
    -> typed Turnstone Action
    -> data-only App state
    -> place/comms/content Effect
    -> shell-owned service port
    -> typed Update
    -> canvas, pane, Knot, and observation projections
```

The shell owns Murm, Gemot, Commons, Knot, transport, and store handles. `App`
holds snapshots, ids, status, and user-visible failures. A peer operation is
authorized by converged Personae/Gemot evidence before it changes the current
projection. Session admission, transport identity, and relay identity never
become content authority.

A Turnstone session may be:

- **personal:** the current local graph shape, usable with peer features
  absent;
- **shared:** the same durable session shape plus a binding to one Moot, one
  root Commons container, its channels, and its Knot documents.

The first implementation uses a versioned `place.json` session sidecar. This
keeps the peer binding out of `graph.json`, preserves local sessions, and
matches Turnstone's existing browser, pane, window, and workbench sidecars.

## First proof: one shared place

Do not begin with a general social shell. Prove one complete place.

Two Turnstone processes, under distinct Personae roots:

1. open the same founded Moot through an invitation;
2. materialize the same shared root graph;
3. exchange one `commons.message` in one channel;
4. attach one HTTPS address to the shared graph and open its live web surface;
5. open one shared Knot document, author one accepted revision from each peer,
   and show the same derived document;
6. stop one process, author while it is absent, restart it, and converge;
7. reject one unauthorized graph write before it changes the projection.

The headed receipt must show both windows and serialize the following
machine-readable facts:

- stable Personae roots and Moot id;
- root container and channel ids;
- accepted operation ids for the message, graph edit, and Knot revisions;
- sync and pending-history state;
- authority outcome for the rejected write;
- identical final graph, channel, and Knot projection digests.

This is the first claim that Turnstone is a peer browser. Existing domain,
loopback, and hardware receipts remain supporting evidence.

## Build ladder

### P0. Product frame - DONE 2026-07-28

Update the README, package description, and active architecture plan. Keep
current implementation status explicit.

Receipt: the product is described as a peer-addressed authoring browser,
Murm/Moot are no longer classified as generic long-tail features, and the
current lack of a Turnstone place port remains explicit.

### P1. Admit the shared-place packages

Move `commons-spine`, rather than copying it, from `mere/crates/probes` into
Mere's workspace under the Moot family. Preserve its graph, chat, call-control,
durability, carrier, and authority tests. Add the smallest Gemot-backed
authority adapter that answers the existing typed Commons capability query.

Turnstone adds explicit dependencies on `comms`, `murm`, `gemot`, the promoted
Commons package, and the required transport package from `mere.git`. The
default `mere` facade remains peer-free.

Done when:

- the old probe path is gone;
- the promoted package passes the existing test corpus unchanged;
- a Commons write authorized by a live Gemot delegation becomes effective;
- revocation withdraws the projection without deleting the retained fact;
- Turnstone compiles with peer support feature-gated and disabled by default.

### P2. One place port

Add `src/place.rs` as the Turnstone adapter and shell-owned worker. Add
data-only place snapshots to `App`, typed actions/effects/updates, and the
versioned `place.json` binding.

The detailed contract, substrate gates, session-truth split, and file order
are fixed in the
[Turnstone place-port plan](./2026-07-28_turnstone_place_port_plan.md).
The port defines a versioned `PlaceInvite` host envelope carrying the signed
Gemot bootstrap artifact, recipient-bound group-key welcome, distinct Commons
root and chat-space ids, default channel id, and carrier-tagged rendezvous.
The envelope is never itself membership or content authority.

The first vocabulary is deliberately small:

- open or join a place;
- send a place message;
- share the focused node;
- open and save a shared Knot document;
- resync and report status.

Done when a render-free two-peer test completes all seven steps of the first
proof, including restart and authorization refusal.

### P3. Headed product receipt

Bind existing surfaces instead of founding a new shell:

- Canvas shows the shared root graph.
- Roster shows Moot members and their authority state.
- Comms shows the active place channel and retains a separate direct-Murm
  scope.
- Workbench opens attached web resources and shared Knot documents.
- Steward shows sync, carrier, retention, and rejected-operation status.

Done when one self-driving two-process scenario produces the headed and
machine-readable receipt defined above. At that point peer support becomes a
default Turnstone capability while local-only sessions and offline startup
remain available.

### P4. Live co-browsing

Add ephemeral presence, follow, focus, and pointer/selection signals over a
Murm admitted session. Durable actions such as sharing an address, annotating,
or saving a Knot revision still pass through the place authority and retained
operation path.

Done when either peer can follow or leave the live session without changing
the durable place, and an explicitly shared address remains after both peers
disconnect.

### P5. Protocol and carrier expansion

Prove additional languages one at a time through real consumers:

- Gemini or Gopher as the first non-HTTP published resource;
- Misfin or LXMF as the first foreign exchange language in Comms;
- Retinue as a carrier for unchanged native place operations;
- Outrider propagation as an offered service visible through Steward and a
  Moot hosting commitment.

Each adapter must preserve source protocol identity and provenance. A second
adapter starts only after the prior one has a headed receipt.

## File seams

| Repository | Seam |
| --- | --- |
| `mere` | promote `crates/probes/commons-spine`; add Gemot authority adapter; keep Stickleback and Knot ownership unchanged |
| `turnstone/src/action.rs` | place and comms intents, effects, and typed updates |
| `turnstone/src/app/mod.rs` | data-only active-place and direct-comms snapshots |
| `turnstone/src/place.rs` | Turnstone lowering between domain services and app vocabulary |
| `turnstone/src/shell/` | service handles, wake integration, persistence, and effect execution |
| `turnstone/src/panes/` | active-place scope versus profile-wide direct comms |
| `turnstone/src/knot_authoring.rs` | bind the existing editor to a shared Knot endpoint without moving document authority |
| `turnstone/src/observe.rs` | ids, sync state, authority outcome, and projection digests for receipts |
| `turnstone/scenarios/` | two-process shared-place headed proof |

## Stop rules

- `Commons` does not become another product name or application shell.
- Mere's default graph facade remains usable without peer dependencies.
- Turnstone does not assemble p2panda sessions or reimplement domain folds.
- A live co-browsing session never becomes durable authority.
- Direct Murm posts and Commons channel messages remain distinct grammars.
- Foreign protocols retain their own semantics and provenance.
- Calls remain at their separately authorized milestone. This plan does not
  start media, device, or codec work. Its A1 consumer is Turnstone after the
  place port exists.
- Protocol expansion waits for the first headed shared-place receipt.
- A green domain test is not reported as a Turnstone product receipt.
- Graphshell may project a place but does not own or host its product state.
