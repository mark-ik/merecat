# The recycle bin: node deletion through eidetic + athanor

2026-07-20, with Mark. How "forget this page" works in merecat: stage the
removed node in the memory subsystem, recover it with its identity intact,
and let athanor (the oven) permanently forget it later, on command or
schedule, baking an engram on the way out.

This resumes work meerkat already did and merecat forgot. It extends the
[Alembic implementation plan](../../mere/design_docs/mere_docs/implementation_strategy/2026-06-24_alembic_implementation_plan.md)
(slice D, Athanor) and the deleted-node bin it names.

## The decision (and the dead ends behind it)

Delete must be recoverable, and recovery must restore the SAME node, not a
stranger that happens to share a url. Three approaches were tried and rejected
before this one, each recorded so the reasoning survives:

1. **App-side tombstone list + a `tombstones` sidecar.** Rejected: invented
   merecat state that leverages none of the stack, plus a sidecar. (Mark: "no
   tombstones", "no dang sidecars".)
2. **Derive "removed" from the graph's navigation memory** (remembered but
   absent). Stack-native, but recovery mints a NEW uuid, so edges/identity are
   lost and the derivation papers over the resulting duplicate by url. (Mark:
   "recover the node with its original uuid.")
3. **A first-class `Node.removed` soft-delete flag** in the kernel. Preserves
   identity, but the marker's home is the memory subsystem, not the node.
   (Mark: "Eidetic/athanor make sense to me.")

The chosen model: the bin lives in **eidetic**; the oven is **athanor**.

## The mechanism already exists

`eidetic::deleted` (eidetic-core/src/deleted.rs) IS the recycle bin:

- `DeletedNode { node_id, url, title, tags, graph_id, deleted_at_ms }` —
  captures the kernel uuid, so recovery can restore the original identity.
- `record_deleted(store, &DeletedNode) -> ManifestId` — stage into the bin.
- `list_deleted(store) -> Vec<DeletedNode>` — read the bin.

meerkat wired this in `node_ops.rs` (`record_deleted` / `list_deleted` +
`run_forgetting_pass`) over an eidetic store opened in `main.rs`. It died with
meerkat; merecat never re-derived it. There is no `purge` / `restore` in the
API by design: the bin is append-only, and "still deleted" is `list_deleted`
MINUS the nodes currently present in the graph — a recovered node reappears in
the graph and drops off the list on its own.

## The slices

### Slice 1 — the eidetic store, as an async port
merecat opens an `eidetic_fjall::FjallStore` at the session dir
(`sessions/<id>/bin/`), behind an armillary actor, mirroring the fetch port:
`Effect::{RecordDeleted, ListDeleted}` -> actor command -> `Update::DeletedListed`.
`record_deleted` / `list_deleted` are async; nothing blocks the render thread
(the Alembic plan's "async, never synchronous on render" rule for athanor).

### Slice 2 — delete, recycle, recover
- **Delete** (`Action::DeleteFocusedNode`, Del key + palette): build a
  `DeletedNode` from the focused node (uuid, url, title, tags, session graph
  id, now-ms), `record_deleted` it (Effect), then remove the node from the
  graph. Close its content session and workbench tile.
- **The Removed section** (Trail, later a dedicated Recycle view): the cached
  `list_deleted` result minus nodes currently in the graph.
- **Recover**: re-mint from the `DeletedNode` record. Restoring the ORIGINAL
  uuid (not `canvas::recover_node`'s fresh-mint) is the refinement — it needs a
  canvas `recover_with_id(uuid, url, title, tags)`. Edges are not restored
  (they left with the node); full-subgraph capture is a later fidelity step.

### Slice 3 — the oven (later)
athanor's forgetting pass permanently drops staged records and bakes an engram
on the way out (distill before forget), on command ("empty the bin") or the
steady-heat schedule. Config knobs land in Apparatus, live passes in Steward
(the Alembic plan's §8 answer). athanor is currently pure pass-logic with no
live consumer; this is where merecat becomes its first.

## Status

Design locked 2026-07-20. Interim hard-delete #4 (merecat b152ed8) unwound
back to the clean quick-wins state (a5ae48a). Slice 1 is next.
