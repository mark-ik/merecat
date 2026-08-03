# Platform families audit — what reaches the product, what doesn't

**Date:** 2026-08-03
**Status:** audit, verified against the live checkouts. Companion to the three
scopes written the same day (engine adoption, Reticulum browsing, user-agent
taxonomy). The question answered: of the platform families Mark listed
(physics, graph primitives, inference and harnessing, the personal mesh), which
already reach Turnstone, which are scoped elsewhere, and which are genuinely
unowned.

## Physics (conatus: numen → quint → seiche) — wired, no scope needed

The canvas consumes it today: `mere-canvas` carries seiche live force physics
and quint, `mere-arrangements` owns the deterministic layouts (aperiodic,
fractal, spiral, axial, radial, grid, semantic-embedding) with seiche
explicitly named as the live-physics owner. Turnstone inherits all of it
through `mere::canvas`. Not a straggler.

## Graph primitives — scoped 2026-08-03, Turnstone is a host

The [graph view curation and interaction plan](../../mere/design_docs/mere_docs/implementation_strategy/2026-08-03_graph_view_curation_and_interaction_plan.md)
(same day, other agent) covers pull/solver response, selection and relation
inspection, folding, temporal scrubbing, and sharing curated views, across
root Canvas, Swatch, and remote Graphshell projection. Turnstone is one of its
three hosts. Nothing to add here; work rides that plan.

## Personal mesh (retinue family) — actively scoped, R1 in progress

The [reachability rungs and privacy lanes plan](../../mere/design_docs/mere_docs/implementation_strategy/2026-08-03_reachability_rungs_and_privacy_lanes_plan.md)
owns the resolver ladder (mDNS, cached address, relay, holepunch, radio) with
R1 in progress, and `mere-transport` carries the feature-gated retinue lane
(trusted-mesh only until retinue R8/R9). Radio management surfaces as a
graphshell adapter. The Turnstone-visible pieces (reachability status in
Steward, the place-lane reconnection gap pinned under T3c) arrive through
those plans. Not unowned.

## Inference and harnessing (intel family) — the genuine straggler

The finding: `crates/intel` holds five built crates (vates, sibylla,
mere-embed, mere-infer, mere-signals) and `eidetic-search` sits beside them,
and **none of them has a consumer outside the family** (verified: no other
crate manifest in mere or Turnstone references any of them). Libraries
without a product lane.

What is scoped for them:

- **Knot search S0/S1** (in the
  [knot-in-graphshell plan](../../mere/design_docs/mere_docs/implementation_strategy/2026-08-02_knot_in_graphshell_plan.md)):
  a real tantivy BM25 index over Knot documents fused through
  `eidetic_search::fuse`. This is the nearest concrete consumer and the
  right first pull: it retires the hash-bucket
  `LexicalEmbeddingProvider` defect and gives sibylla's retrieval core a
  production caller.
- **Eidetic browsing derivation** (2026-06-12 plan) is the older consumer
  vision (derivations over browsing truth); it predates the consolidation
  and should be re-read before anything builds against it.
- **Burn** is the endorsed strategic direction for the model lanes; nothing
  here proposes alternatives.

What is not scoped anywhere: a harnessing surface in the product (vates
driving anything a user sees: summarization, suggestion, an assistant lane),
and mere-signals' role. Recorded as open rather than planned, deliberately:
the first inference consumer should be the already-scoped search fusion, and
a product assistant lane deserves its own design conversation with the
participant gate in the room (an inference actor is a participant like any
other, so the gate model already names its admission path).

## Also checked in this sweep

- **Smolweb homes**: decided and recorded 2026-08-03
  ([smolweb home decision](../../mere/design_docs/nematic_docs/technical_architecture/2026-08-03_smolweb_home_decision.md)).
- **Engines, Reticulum browsing, UA taxonomy**: scoped the same day in this
  directory.
- **Eepsites**: the I2P transport skip stands as recorded in mere-transport;
  when it reopens it enters through the engine plan's E4 seam like the
  Reticulum lane.

## The one recommendation

Sequence the intel family's first consumer (Knot search S0/S1) ahead of any
new inference scoping. It is already planned, it is small, and it converts
the family from zero consumers to one real one, which is the fact every
further design conversation wants to stand on.
