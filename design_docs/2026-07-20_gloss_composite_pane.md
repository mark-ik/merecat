# Gloss as a composite pane; the swatch as a customizable projection

2026-07-20, Mark's ask (during the recycle-bin work): "at base it's a
minimap, but if you wanted to you could put sections from other panes in
there too (like recent nodes, or deleted)" — and the swatch itself "can and
should be customizable... must consider the UI for this." Ruled POSSIBLE;
this note records the shape so the build starts from it. Design capture
only; nothing here is implemented.

## Why it is structurally cheap

Both halves already exist as the right kind of pieces:

- **Sections are already the currency.** Trail is a `sectioned_list` over
  neutral `ListSection`s, and every pane's content is gathered by a pure
  `fn(&App) -> rows` (trail_rows, the roster gather, the recycle bin's
  removed-derive). Naming these — a **section-provider registry**, id →
  gather — makes any list section composable into any pane. The providers
  are the functions we already have.
- **The swatch is already a projection.** The Gloss minimap is a sprigging
  custom-paint leaf over a cartography projection of the graph. "Customizable
  swatch" = the projection's parameters (color-by, filter, emphasis, layout
  source) become a named preset the pane config carries — the register-lens /
  cartography vocabulary, not new machinery.

## The shape

- `GlossConfig { sections: Vec<SectionId>, swatch: ProjectionPreset }` —
  ordered, the swatch being section zero by default.
- **Config lives ON THE FRISKET LEAF** (arrangement truth): it persists with
  `frame.json`, tears out with the pane, differs per lens window, and needs
  no sidecar. Precedent: `PaneContent::Tile(uuid)` already carries data.
  Likely `PaneContent::Gloss(GlossConfig)` with serde defaults so old
  layouts load.
- Composition generalizes: if it proves out on Gloss, ANY list pane can host
  foreign sections (Trail's Removed inside Gloss, a roster bucket beside the
  minimap). Gloss is the pilot because its identity ("the navigator") is
  additive, not replaced.

## The UI question (the real work, per Mark)

Candidates, not yet chosen:

1. **Right-click the pane → config actions in the palette** ("Gloss: add
   section — Recent", "Gloss: remove section — Removed"). Rides the
   just-landed right-click palette; zero new chrome. Weakness: invisible
   affordance.
2. **An edit mode on the pane** (a small ⚙ row when the pane is active):
   in-place add/remove/reorder rows. Needs the pane-header furniture panes
   deliberately don't have yet.
3. **Drag a section header from one pane into another** (the tile-drag
   grammar applied to sections). The most direct manipulation, the most
   gesture work; fits the tear-out family long-term.

Recommendation when built: start with (1) — it is spine-honest, receipt-
drivable, and teaches the palette scoping we already want; grow (3) once
section headers are draggable objects. NOT Apparatus: pane composition is
arrangement/view state, not graph-object metadata (the taxonomy holds).

## Prerequisites

- The section-provider registry (extract the existing gathers behind ids).
- `PaneContent::Gloss` grows its config (serde-defaulted).
- Palette actions scoped to the active pane (the registry's category reread
  the harvest doc already earmarks).

## Progress

- **2026-07-21 (the preset half LANDED — merecat `b02025c`):**
  `src/swatch_pane.rs` is the vocabulary: `ProjectionPreset` (id, label,
  leaf key, component knobs, one pure `gather: fn(&App) -> SwatchModel`) and
  the one retained `SwatchPane` over any preset. Activation rides each node
  as DATA (`SwatchActivate::Open/Switch`), so presets need no handler code
  and the shell lowers every variant through the spine. Gloss + the Overmap
  are now `GLOSS_MINIMAP` / `OVERMAP_LINEAGE` preset consts of the one pane;
  receipts re-ran green (rung5_gloss's `click-node` proves the probe contract
  survived the refactor). Riding along: cambium's swatch grew `with_expand` /
  `with_node_labels` knobs (the overmap labels its sessions), and pane
  pointer-move routing landed (`deliver_hover` — the hover emphasis the
  component always supported now lights up). **Remaining**: the sections half
  (the provider registry, `PaneContent::Gloss(GlossConfig)`, and the UI
  question above — untouched, still the real work).

- **2026-07-22 (the sections half, slice 1 LANDED — merecat `151b416`):**
  `src/sections.rs` is the section-provider registry: `SectionProvider` (id,
  title, a pure `gather: fn(&App) -> Vec<SectionRow>` — the same currency as a
  preset's `gather`, the seam the swatch half left), with `RECENT_SECTION` /
  `REMOVED_SECTION` (the Trail's Recent/Removed gathers, now composable) and
  `by_id` lookup. `ProjectionPreset` grew a `sections: &[SectionProvider]`
  field: `GLOSS_MINIMAP` composes `[REMOVED]` (deleted nodes are gone from the
  graph, so the minimap is where you look for them), the Overmap composes none
  (it fills). The swatch shrinks to the top `SWATCH_FRACTION` and the sections
  stack below in a scrollable column; `swatch_view` renders title + rows,
  inert display. Receipt `rung5_gloss_composite.scn` headed ok (the Removed
  row shows under the minimap after a delete); `rung5_gloss` re-run green (the
  `click-node` probe survived the swatch shrink). The registry test caught a
  real semantic: the Removed section filters by ORIGINAL node id, so a plain
  open (new id) does not clear it — only recover (which restores the id) does.
  **Remaining (slices 2+):** a section row's click (a Recent row navigates, a
  Removed row recovers — the swatch's `resolve`/probe contract extended to the
  section rows); the per-frisket-leaf config (`PaneContent::Gloss(GlossConfig)`
  so a pane chooses its sections, persisted with `frame.json`); and the
  add/remove UI (the right-click palette scoped to the active pane).

- **2026-07-22 (the sections half FINISHED — merecat `4afed14`):** all three
  remaining slices.
  - **Rows are live.** `SectionRow` carries a `SectionActivate` (Open /
    Recover) as DATA, like a swatch node's, so a provider declares what its
    rows mean and needs no handler code. Recent navigates; Removed recovers by
    ORIGINAL id (the recycle bin's identity contract — the Trail and the Gloss
    agree). Rows wear a `section-row` class and `click_pane_row` learned it, so
    one verb addresses a composed row wherever it was composed. (The first
    headed run missed exactly there; the selector was the fix.)
  - **Composition rides the leaf.** `PaneContent::Gloss(GlossConfig)` carries
    provider ids, so a pane's composition persists with `frame.json`, travels
    on tear-out, and differs per lens window. Sections moved off
    `ProjectionPreset` (slice 1's hardcode) to the pane instance:
    `sections::resolve` maps the leaf's ids to providers each frame, SKIPPING
    unknown ids so a newer build's config degrades instead of failing, and
    `SwatchPane::set_sections` hands them over. frisket grew
    `content_mut(pane_id)` — the seam for editing a leaf's own config in place.
    The unit→tuple variant change is the deliberate no-legacy-friction cut
    (DOC_POLICY §3): a pre-`GlossConfig` layout resets to default, logged.
  - **Add/remove is pane-scoped palette rows.** While a Gloss is active the
    palette offers `Gloss: add/remove section — <Title>` per provider;
    `Action::TogglePaneSection` edits that leaf and persists. Rings as `Panes`
    (composition edits the layout, not a node). At base a Gloss is a bare
    minimap — you compose it.
  - Receipt `rung5_gloss_composite.scn` headed ok end to end: bare minimap →
    compose Removed → click the row to recover by original id → toggle back to
    bare. 107 unit tests.

  **What this generalizes to (not built):** the registry is pane-agnostic, so
  any list pane could host foreign sections — the design's original "Trail's
  Removed inside Gloss, a roster bucket beside the minimap". Only Gloss reads a
  config today. Also open: section ORDER (config order is honoured but there is
  no reorder affordance), and the drag-a-section-header gesture (UI candidate 3,
  which wants draggable section headers first).

- **2026-07-23 (the open bits: reorder + the roster bucket):** two of the three
  items above.
  - **Reorder.** `Action::MovePaneSection { pane, section, delta }` moves a
    section within its pane's stack. Composition order IS the config's order, so
    a reorder is the same leaf edit as add/remove: same clamped-in-bounds rule,
    same `SaveSession`, same `Panes` ring. It clamps at the ends rather than
    wrapping, because a stack has a top and a bottom and a silent wrap would
    read as a bug. The palette only offers a move that would do something (no
    rows at all with one section, no "up" on the first), so every offered row
    changes the pane. A no-op move reports nothing, which keeps the receipt
    honest: `pane-section-moved` fires only on a real move.
  - **The Nodes provider.** `NODES_SECTION` gathers the graph's nodes,
    most-recently-visited first, capped at 8. This is Mark's own example ("a
    roster bucket beside the minimap"): a section borrowed from a pane the
    Gloss is not. It cost one `fn(&App) -> Vec<SectionRow>` and one line in
    `ALL`, which is the registry paying off as designed.
  - Receipt `rung5_gloss_composite.scn` extended end to end: compose Removed,
    recover from it, compose Nodes beside it, move Nodes up, then remove both
    back to the bare minimap.

- **2026-07-23 (the host side GENERALIZES — the third open bit):** composition
  is now a property of a PANE, not of the Gloss.
  - `GlossConfig` became **`PaneComposition`**, and `PaneContent::Overmap` grew
    one. `PaneContent::composition()` / `composition_mut()` are the single place
    that answers "does this pane compose?", so gaining a host is a variant
    listed there rather than another match spread through the host.
  - The Overmap was the honest second host because it already renders through
    the SAME `SwatchPane`: it cost a `sections::resolve` and a `set_sections` in
    its render arm, no renderer work at all. That is the swatch-half author's
    seam paying off twice.
  - `pane_section_actions` is written against `composition()` rather than a pane
    kind, and its row prefix is the pane's own tag, title-cased. So the Overmap
    named itself ("Overmap: add section — Removed") with no second table, and
    the Gloss's existing labels stayed byte-identical (the older receipt still
    passes unchanged).
  - Worth stating because it looks like a contradiction: the Overmap is
    window-chrome (`follows_active_graph() == false`) while its composed
    sections read ACTIVE-GRAPH truth. That is consistent. A section gathers from
    app state each frame; `follows_active_graph` governs whether the host
    retags the LEAF's `graph_id`, which sections never consult.
  - Receipt `overmap_composite.scn` headed ok: base swatch, compose Removed,
    compose Nodes, reorder, then click the composed Removed row to recover the
    node by its ORIGINAL id from inside the Overmap. 109 unit tests + frisket's
    17.

  **The drag gesture stays deferred, with a reason.** Merecat already has a
  full tab-drag grammar (`WorkbenchStackOnto`, `WorkbenchSplitBeside`,
  `TearOutTile`), but it lives in the WORKBENCH lane and is tile-shaped. A
  section-header drag would either duplicate that machinery inside the swatch
  pane or wait for it to be generalized into a shared "drag an object between
  panes" lane. Duplicating it is the mistake the component catalog exists to
  prevent, so this waits on the generalization rather than growing a second
  bespoke drag lane. The cheap half (headers as identified, hit-testable
  objects) is already there in spirit: rows wear `section-row` and the resolver
  knows it, so a header class is a small step whenever the lane arrives.
