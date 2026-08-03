# Reticulum browsing — NomadNet nodes and capsules, idiomatically

**Date:** 2026-08-03
**Status:** scoped with Mark. Doctrine, stated by him for this lane and
matching the smolweb one: be honest about the content, optionally enrich the
presentation, and never collapse anything into formats that are not idiomatic
to the protocol.

**Relation to the peer-web reframe:** [P5](2026-07-28_turnstone_peer_web_reframe.md)
governs protocol/carrier expansion with a one-adapter-at-a-time rule and
headed receipts. This plan scopes the browsing lane so it is ready to enter
that queue; it does not jump it. Retinue-as-place-carrier and LXMF-in-Comms
are P5's other rows, not this plan.

## What exists (verified 2026-08-03)

- **retinue** (the Reticulum family workspace, MPL-2.0): links, announces,
  resources, destinations, all RF-proven on real hardware.
- **`gemini_over_reticulum` example** (retinue/crates/retinue/examples):
  a gemtext capsule served and fetched over an encrypted link with named
  announce-based addressing (`gemini://capsule/` resolved by
  recompute-and-match against announces, the NomadNet addressing model) and
  faithful end-of-response semantics (`LinkStream` shutdown maps to link
  close, the reader sees clean EOF). The bytes are exactly what
  `errand::gemini_exchange` writes and reads.
- **The gemtext lane end to end**: errand's gemini parse and the
  cambium-nematic views render gemtext today. For gemini-over-Reticulum,
  parse and render are already built; only the carrier differs.
- **No micron anything.** No parser, no view, no node-page client, anywhere
  in the workspace.

## The two content families

### 1. Gemini over Reticulum (near-free)

Same gemtext, different carrier. The transport swap is retinue's LinkStream
in place of TLS/TCP; everything above the bytes is the existing lane.

The one real design item is **trust**: there is no TLS and no TOFU pin.
The destination identity is the authority; a named capsule is whichever
announcer's identity yields that destination. That maps into the fidelity
plan's trust descriptor as its own posture (identity-addressed, stronger
than TOFU in one way, name-squattable in another: first-announce wins a
name the same way first-connect wins a pin). Surface it honestly in the
tile chrome; never borrow the TLS vocabulary.

### 2. NomadNet nodes (the real work)

NomadNet nodes serve **micron** pages (`/page/index.mu`) and files over
Reticulum request handlers, bodies delivered as Resources (windowed,
compressed, explicit COMPLETE; retinue already implements Resources at the
protocol level). Micron is its own markup idiom: sections and depth,
formatting and color codes, links to node pages and files, and input
fields that submit values back to the node.

Micron renders **as micron**. Its sectioning, alignment, and color model
survive to the screen; links and input fields keep their submit-to-node
semantics. Lowering it into gemtext or HTML first would be exactly the
collapse the doctrine forbids. Enrichment (theming, focus affordances,
history integration) rides on top, optional and visibly ours.

**Clean-room note:** Nomad Network is GPL. The micron implementation is
written from the published markup documentation (NomadNet ships a markup
reference as user-facing docs), not from its source, per the sennet
clean-room posture. The retinue workspace stays MPL-2.0.

## Homes (per the smolweb home rule, applied to this family)

The [smolweb home decision](../../mere/design_docs/nematic_docs/technical_architecture/2026-08-03_smolweb_home_decision.md)
generalizes: spec-accurate implementations go to the protocol family's
general-use workspace; enrichment and rendering stay ours.

- **retinue workspace**: the micron parser (spec-accurate, general-use) and
  the node-browsing client (page/file requests over links + Resources,
  announce-based name resolution). These are Reticulum-family facts, so
  they live with the trunk, usable by anyone without our stack.
- **cambium**: the micron view, beside cambium-nematic's gemtext/gopher/feed
  views. Rendering is implementation-specific by the rule.
- **turnstone**: the lane wiring, through the
  [engine adoption plan](2026-08-03_turnstone_engine_adoption_plan.md)'s
  seams: a session engine whose fetch runs over retinue, appearing in the
  picker like any lane, with E4's no-handler fallback covering the schemes
  until it lands.

## Steps

### N0. Addressing and scheme decision

Decide how a Reticulum resource is written in the omnibar and stored in
graph nodes: direct (`<scheme>://<dest-hash>/page/index.mu`) and named
(announce-resolved) forms, and what the scheme string is for nomadnet nodes
versus gemini capsules. Constraint: a stored address must survive restart
and re-resolve, so the durable form carries the destination hash with the
name as annotation, not the reverse. Small decision, gates everything.

### N1. Gemini over Reticulum, end to end

Promote the example into a Turnstone lane: errand-shaped fetch over
LinkStream behind the session-engine fetch seam, existing parse and views,
trust posture surfaced. Headed receipt: a capsule served from a second
process (or second machine) renders in a Turnstone tile.

### N2. Micron parser (retinue workspace)

Spec-accurate line-level parse to a micron AST: sections/depth, formatting
and color spans, alignment, dividers, links, input fields. Unit-tested
against the markup reference's own examples. No rendering, no transport.

### N3. Node browsing client (retinue workspace)

Page and file requests against a node destination over a link, Resource
delivery, name resolution via announces. The client returns bytes plus
provenance (destination hash, announced name, delivery completeness).

### N4. Micron view (cambium)

The micron AST rendered natively: sections as structure, colors and
formatting honored, links navigable, input fields that build the submit
request. Same element-tree testing pattern as the smolweb views.

### N5. Turnstone lane + receipt

Register the lane, route the scheme(s) from N0, wire history/graph capture
so a browsed node page is a node in the graph like any other address.
Headed receipt against a real NomadNet node (or our own node stood up from
the retinue side) on the LAN.

## Not in scope

- **LXMF messaging.** Outrider's lane, enters through Comms under P5.
- **Serving our own pages** (the authoring/hosting side). Real, later; the
  browsing lane comes first.
- **Propagation nodes and offline delivery.** Retinue roadmap, not browsing.
- **Enrichment beyond presentation** (graph annotations, illume passes over
  micron prose): welcome once N4 stands, settings-gated per the
  configurability rule.

## Ordering

N0 then N1 gives a real Reticulum browsing receipt for the cost of a carrier
swap, and exercises the trust posture early. N2 to N4 are parallelizable
after N0; N5 closes. P5's one-at-a-time rule decides when this enters the
build queue relative to the other protocol adapters.
