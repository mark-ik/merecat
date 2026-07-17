# Merecat's surfaces expressed in cambium

2026-07-15. With cambium adopted (the Roster grid landed, d05f24d), every merecat
surface gets asked the same question: what cambium component expresses it? The
answer is one of three — an existing catalog entry, a new catalog addition, or
"stays non-cambium." This doc is that mapping.

The framing is the serval/genet-era development pattern: **a consumer's need is a
good reason to expand the catalog.** Not the only reason — cambium has its own
sense of what belongs — but a real one. So where a pane wants a primitive cambium
lacks, the addition is named and justified by the consumers that pull it, and the
strongest additions are the ones several panes pull at once.

## The seam (recap)

A cambium view renders into a `ScriptedDom`; merecat lays that out with
genet-layout and composites it at the pane's surface rect (`ui::scene_from_dom`
under a host sheet). Events feed into the view's `GenetAppRunner`, which returns
Actions merecat lowers through its spine. So "express surface X in cambium" means:
X's content is a cambium view over a `GenetAppRunner`, composited at X's rect. The
host keeps the surface plan, the compositor, and the canvas; cambium owns what a
pane draws inside its rect.

## The mapping

| Surface | cambium expression | Status |
| --- | --- | --- |
| **Chrome / omnibar** | `text_field_typed` + `action_list` (the find lane) / `command_surface` (the `>` lane) | existing — migrate |
| **Caption chip** | `el` (a positioned label) | existing — trivial |
| **Roster** | `data_grid` + **tab strip** (Nodes/Links/Graphlets/Fields) + `detail_popover` (facet cards) | grid DONE; tabs NEW |
| **Trail** | **sectioned list** (Recent/This-node/Removed) + `button` (Recover) | list NEW |
| **Inspector** | **detail panel** (key/value sections, diagnostics) | NEW |
| **Gloss** | `graph_canvas_swatch` (minimap) + **tree/outline** (structure) | swatch existing; tree NEW |
| **Steward** | **sectioned list** + `Meter` leaf (progress) | list NEW; Meter existing |
| **Comms** | **message list** (chat) | NEW |
| **Alembic** | **sectioned list** / `data_grid` (Recent/Saved/engrams) | reuses Trail/Roster |
| **Apparatus** | `checkbox` / `toggle` / `radio_group` / `select` / `slider` / `text_field_typed` (settings) + **sectioned list** (diagnostics) | controls existing |
| **Workbench** | **tab strip** (tile tabs) + **split** (tiling) + `Swatch` leaf (node bodies) | swatch existing; tabs/split NEW |
| **Notes / knot editor** | `editor` / `styled_textarea` | existing |
| **Pane furniture** (frisket dividers, stacked-pane tabs, maximize/close) | **split** + **tab strip** | NEW |
| **Orrery (the canvas)** | stays `mere::canvas` | non-cambium |
| **Content documents** (live pages) | stays genet document sessions | non-cambium |
| **Surface plan / layered present** | stays merecat (host composition) | non-cambium |

## The catalog additions the consumers justify

Ranked by pull — how many merecat surfaces want each. The multi-consumer ones are
the real catalog candidates; the single-consumer ones can compose from existing
primitives first and graduate to the catalog if a second consumer appears (the
family's crate-promotion gate, applied to components).

1. **Tab strip** — the strongest. Roster's four data tabs, the Workbench's tile
   tabs, and any stacked frisket panes all want the same "one strip of labeled
   tabs, one active, click/keys to switch" widget. Three distinct consumers, one
   shape. cambium has `keyed` for the reconciliation and `arrangement` for
   placement; the tab strip is the composition worth naming.

   **LANDED 2026-07-17** — `cambium::tab_strip` (catalog: a tab strip) + the
   Roster's four tabs (Roster: tabs, from cambium's catalog). Two things the
   first consumer pulled that the sketch above did not predict:

   - **Generic over `Action`.** The strip emits none (switching a tab is a state
     change), but its siblings do — the Roster's grid bubbles a `Navigate`. A
     `()`-actioned strip would force every such caller through `map_action`, so
     the strip is generic like `data_grid`, not `()`-actioned like the controls.
   - **The host owns the strip's geometry, so the host must state its height.**
     The strip sets none by design. That makes `TABLIST_HEIGHT` a host-side
     restatement of a sheet fact, which is a drift risk, so it is test-held. A
     tab's *x* cannot be restated at all (flex + text measurement), so the host
     asks `absolute_rect` rather than computing. **Any pane that composes a
     cambium widget inherits this shape**: the widget's geometry is knowable only
     from the layout, so ask it.

2. **Split / divider pane** — the pane furniture. Today merecat hand-computes pane
   rects in `pane.rs` and has no divider drag. A cambium `split` (two children, a
   draggable divider, a ratio) owns the resize gesture and the seam, and both the
   frisket pane tiling and the Workbench's platen tiling pull it. This is the
   shell's tiling chrome becoming a component — the largest single migration, and
   the one that shrinks `pane.rs` to a walk over cambium splits.

3. **Sectioned list** — grouped rows under section headers, each row navigable or
   a button. Trail (Recent/This-node/Removed), Steward (active/queued), Alembic
   (Recent/Saved), and Apparatus's diagnostics all want it. Either a new `list`
   with sections or `action_list` grown a section grouping. Four consumers; a
   catalog primitive.

4. **Tree / outline** — a collapsible hierarchy. Gloss's document outline pulls it
   first; Roster's Graphlets tab and any settings tree follow. Two-plus consumers.

5. **Detail panel** — a pane-filling structured read view: labeled key/value rows
   in sections, some values rich (a trust badge, a parse-diagnostic list).
   `detail_popover` exists but is a transient popover, not a resident panel;
   Inspector and Roster's facet cards pull the resident form. Two consumers.

6. **Message list** — Comms's chat. One consumer, distinctive shape; compose from
   `el` + `arrangement` until Moot gives it a second.

Everything else a pane needs already exists: the `data_grid`, the full control set
(`button`, `checkbox`, `toggle`, `radio_group`, `select`, `slider`,
`text_field_typed`), the editors (`editor`, `styled_textarea`), `menu`,
`overlay_surface`, `detail_popover`, `command_surface`, and the sprigging leaves
(`GraphCanvas`/`graph_canvas_swatch`, `Swatch`, `Meter`, `Knob`, `GraphGlyph`).

## What stays non-cambium, and why

- **The Orrery canvas.** `mere::canvas` is the graph-truth-plus-physics
  presentation library — selection, layout, the arrangement geometry sidecar. It
  is not a widget; it is the space-view. sprigging's `GraphCanvas` leaf is the
  right tool for an *embedded* graph (a Gloss minimap, a swatch), not for the full
  interactive canvas. The canvas surface stays mere's, composited beside the
  cambium panes.
- **Live content documents.** genet's document sessions render web pages to a
  `netrender::Scene`. That is the engine, not the toolkit; it stays.
- **The host composition** — the surface plan, the compositor, event routing into
  each pane's runner. This is merecat's job as the host; cambium owns pane
  interiors, not the window.

So the boundary is clean: **cambium owns what is inside a pane rect; merecat owns
the rects, the canvas, the documents, and the composition.**

## Sequencing

The order follows pull and dependency, not pane-by-pane:

1. **Finish Roster on the grid** — event dispatch (`runner.dispatch_click`), which
   also proves the general pane-event path every cambium pane reuses.
2. ~~**Tab strip**~~ — DONE 2026-07-17. Unlocked Roster's four tabs; the first
   multi-consumer catalog addition. Only Nodes has a gatherer; Links / Graphlets
   / Fields say so until the edge-family walks land (no general edge iterator —
   `semantic_edges` / `arrangement_edges` / `containment_edges` each need one).
3. **Gloss** — minimap half DONE 2026-07-17: `graph_canvas_swatch` on the new
   leaf pipeline (`scene_from_dom_with_leaves` — sizes each `<custom-leaf>` from
   its laid-out box, repaints dirty leaves through the pane's `LeafRegistry`,
   splices at the box). Data via `Canvas::minimap_geometry`; node colour from
   mere's palette (NODE_SHEET carries into the minimap); a node click drains a
   Navigate intent -> `OpenAddress`. NOTE the swatch's interaction contract is
   **mirror-then-drain, not action bubbling**: its handlers are `Fn(&mut State,
   Id)` mutators, so the pane records intents in its own state and the shell
   drains them after dispatch — the tab strip's shape, not the grid's. The
   outline half still pulls **tree** when outline data lands.
4. **Split** — migrate the pane furniture; `pane.rs` becomes a walk over cambium
   splits, and divider drag arrives for free.
5. **Sectioned list** — Trail migrates onto it; Steward/Alembic/Apparatus follow.
6. **Inspector** (detail panel), **Comms** (message list) as their data lands.
7. **Chrome/omnibar** onto `text_field_typed` + `command_surface` — the last
   hand-DOM holdout retires.

Each cambium addition is a small PR to the cambium catalog, justified in its
description by the merecat surfaces that pull it — the serval/genet pattern, now
running with merecat as the consumer.
