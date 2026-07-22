# G3 Graphshell Merecat endpoint receipt

**Status:** complete locally on 2026-07-22. Publication remains gated on the
Graphshell donor rename and repository publication.

## What this proves

- Merecat now has a library boundary around its existing read model, action
  reducer, desktop shell, and remote projection adapter.
- The endpoint reads the live Mere graph. Mere cartography maps three stable
  node UUIDs, their recency order, measured card extents, and two graph
  relations into a Scenograph spiral score and scene.
- Graphshell receives the scene plus a presentation sidecar. It fetches and
  verifies content-addressed portable-card resources without receiving the
  graph store.
- The generic Graphshell receipt view realizes the disclosed item origins and
  relations at wide widths. Its narrow representation is a readable semantic
  card stack with the same actions.
- Both advertised intents return through one Servitor gate. The projected
  `projection/layout/` write grant admits `FitView`; the graph-changing
  `OpenAddress` petition under `graph/open/` is rejected. Mere graph node count
  and revision stay unchanged.
- The accepted petition is committed to an ordinary Chartulary audit graph and
  attributed to the endpoint subject. Authority is rebuilt from the
  gate-authored grant projection.

## Acceptance

- `cargo check --lib`
- `cargo test --lib remote_projection -- --nocapture`
- the committed `docs/receipts/g3_merecat_endpoint.html` is compared
  byte-for-byte with a fresh executable run;
- Graphshell `cargo test --workspace` remains green;
- headed inspection at 1180 by 900 and 390 by 844 confirmed the semantic
  cards, keyboard-focusable actions, responsive collapse, and absence of
  horizontal overflow or browser errors.

## Deliberate limits

This is a local loopback endpoint over an in-memory deterministic Merecat
fixture. It supports the Spiral score for this proof. A real session selector,
authenticated carrier, grant negotiation, diffs, revocation, live-pane codec,
and durable Graphshell host store remain later proofs. The graph action is
intentionally refused because this endpoint holds only the layout grant.
