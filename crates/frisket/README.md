# frisket

The pane model for [mere](https://crates.io/crates/mere).

On a hand press the **frisket** is the hinged frame whose cut-out apertures
decide what prints where. This is the same frame, over a window: a savable tree
of resizable panes, projected into a uxtree.

Renamed from `frame` on 2026-07-14. That name was overloaded three ways in this
family (a rendered frame, a `TileFrame`, a window's pane arrangement) and it also
fused in the workspace id vocabulary, which now lives in
[`incipit`](https://crates.io/crates/incipit).

## What it owns

- [`PaneId`], [`PaneContent`], [`PaneNode`], [`FrisketLayout`], [`FrisketId`]: the
  pane kinds, the split tree, and the saved layout.
- The layout operations: `summon_leaf`, `reparent_leaf`, `close_leaf`,
  `split_at`, `set_split_ratio`, and the multi-graph re-sourcing policy
  (`retag_graph_bound`, `dedupe_graph_panes`).
- [`project_frisket`] / [`project_frisket_with`]: a uxtree subtree describing the
  layout, so the pane tree is accessible.
- Serde derives, so a layout survives a restart.

## The tiers

They nest rather than compete:

- **frisket** is pane-ness: kinds, ids, the split tree, the persisted layout.
- **platen** tiles graph nodes *inside* one `PaneContent::Workbench` leaf. Every
  platen tile is a graph node; a frisket pane can be anything.
- **The host** turns this ratio tree into rects. Frisket emits fractions, never
  pixels, and never a rect.

Pane content is named by a [`PaneContent`] tag rather than a typed reference, so
this crate stays decoupled from the crates that supply what a pane shows (gloss,
roster, trail, and the app's own panes).

## Where it lives

A mere crate today, because meerkat depends on it. It is destined for merecat:
`PaneContent` names panes that merecat owns outright (Inspector, Comms), and a
library crate enumerating the app's panes is an inversion. It moves when meerkat
is deleted, and `session-runtime`'s `frisket_store` goes with it.

Persistence is `session_runtime::frisket_store`, which writes `frame.json` beside
`graph.json`. The on-disk tag is still `frame`, deliberately: renaming it is a
format migration, parked together with `PaneContent::Orrery` (a serde variant
name, and so also on-disk) as one vocabulary decision rather than two silent
breaks.

## Status

Pre-1.0. 14 tests. The model, the layout ops, the projection, and the persistence
are landed and in production under meerkat. Geometry, hit-testing, and focus
routing are the host's, and merecat builds them at rung 5.
